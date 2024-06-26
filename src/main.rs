use b2_backblaze;
use chrono;
mod config;
mod error;
use error::Error;
use std::fs;
use std::process::Command;
use clap::Parser;

#[derive(clap::Parser)]
#[command(about, long_about = None)]
struct Arg {
    #[arg(short = 'c', long = "config")]
    config_file: Option<String>,
}

#[tokio::main]
async fn main() {
    let config_file = Arg::parse();
    let config_path = config_file.config_file.unwrap_or_else(|| {
        println!("Using default config location: ./Config.toml");
        String::from("./Config.toml")
    });
    
    let config = match config::read_config(config_path) {
        Ok(config) => config,
        Err(err) => {
            println!("{}", err);
            return;
        }
    };

    if !std::path::Path::new("./backups").exists() {
        match fs::create_dir_all("./backups") {
            Ok(_) => {}
            Err(err) => {
                println!("{}", Into::<Error>::into(err));
                return;
            }
        };
    };

    if !std::path::Path::new("./tmp").exists() {
        match fs::create_dir_all("./tmp") {
            Ok(_) => {}
            Err(err) => {
                println!("{}", Into::<Error>::into(err));
                return;
            }
        };
    };

    println!("Starting backups...");

    for db in config.databases.keys() {
        let backup_config = config.databases.get(db).unwrap();

        let Some(backup_host) = config.hosts.get(&backup_config.db_host) else {
            println!(
                "{}",
                Error::BackupHostConfigError(backup_config.db_host.clone(), db.to_string())
            );

            continue;
        };

        let Some(backblaze_config) = config.buckets.get(&backup_config.bucket) else {
            println!(
                "{}",
                Error::BucketConfigError(backup_config.bucket.clone(), db.to_string())
            );

            continue;
        };

        let timestamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let filename = format!("{}_{}_{}", db, backup_host.db_host, &timestamp);
        let databases = backup_config.databases.as_str();
        let mut databases_argument = "--databases";

        match databases {
            "*" => {
                databases_argument = "--all-databases";
            }
            _ => {}
        };

        println!(
            "Exporting databases {} from {}",
            &databases, &backup_host.db_host
        );

        let output = Command::new("mariadb-dump")
            .arg(format!("--host={}", backup_host.db_host))
            .arg(format!("--port={}", backup_host.db_port))
            .arg(format!("--user={}", backup_host.db_username))
            .arg(format!("--password={}", backup_host.db_password))
            .args([databases_argument, databases])
            .arg(format!("--result-file=./tmp/{}.sql", &db))
            .output();

        match output {
            Ok(output) => match output.status.success() {
                true => {
                    println!("Successfully exported database {}!", db);
                }
                false => {
                    println!(
                        "{}",
                        Error::MariaDbDumpError(
                            String::from_utf8_lossy(&output.stderr).to_string(),
                            db.to_string()
                        )
                    );

                    continue;
                }
            },
            Err(err) => {
                println!(
                    "{}",
                    Error::MariaDbDumpError(err.to_string(), db.to_string())
                );

                continue;
            }
        }

        let result = Command::new("tar")
            .arg("-czvf")
            .arg(format!("./backups/{}.tar.gz", filename))
            .arg("./tmp")
            .output();

        match result {
            Ok(_) => {
                println!("Successfully compressed database {}!", db);
            }
            Err(err) => {
                println!(
                    "{}",
                    Error::DatabaseCompressionError(err.to_string(), db.to_string())
                );

                continue;
            }
        }

        let client = b2_backblaze::B2::new(b2_backblaze::Config::new(
            backblaze_config.application_id.clone(),
            backblaze_config.application_key.clone(),
        ));

        client
            .set_bucket_id(backblaze_config.bucket_id.clone())
            .await;

        match client.login().await {
            Ok(_) => {}
            Err(err) => {
                println!("{}", Error::BackblazeLoginError(err.to_string()));
                continue;
            }
        }

        let upload_location = format!("{}/{}.tar.gz", backup_config.remote_path, filename);

        match client
            .upload(
                upload_location.clone(),
                format!("./backups/{}.tar.gz", filename),
            )
            .await
        {
            Ok(_) => {
                println!(
                    "Successfully uploaded database {} at {}!",
                    db, upload_location,
                );
            }
            Err(err) => {
                println!("Failed to upload database backup: {:?}", err);
            }
        }
    }

    fs::remove_dir_all("./tmp")
        .unwrap_or_else(|err| println!("{}", Error::IoError(err.to_string())));
}
