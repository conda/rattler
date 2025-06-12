use serde::{Deserialize, Serialize};

use crate::config::{Config, ConfigBase};

mod config;
mod edit;

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FoobarConfig {
    pub example: String,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigExtension {
    #[serde(default)]
    pub foobar: FoobarConfig,
}

impl Config for ConfigExtension {
    fn get_extension_name(&self) -> String {
        "foobar".to_string()
    }

    fn merge_config(self, other: &Self) -> Result<Self, miette::Error> {
        Ok(Self {
            foobar: FoobarConfig {
                example: other.foobar.example.clone(),
            },
        })
    }

    fn validate(&self) -> Result<(), miette::Error> {
        if self.foobar.example.is_empty() {
            Err(miette::miette!("foobar.example cannot be empty"))
        } else {
            Ok(())
        }
    }

    fn keys(&self) -> Vec<String> {
        vec!["foobar".to_string()]
    }
}

fn main() {
    // take first element and load as config file in extensible_config
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <config_file>", args[0]);
        std::process::exit(1);
    }

    let config_file = &args[1];
    match config::load_config::<()>(config_file) {
        Ok(config) => {
            println!("Loaded config: {:?}", config);
        }
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            std::process::exit(1);
        }
    }
    match ConfigBase::<ConfigExtension>::load_from_files(vec![config_file]) {
        Ok(config) => {
            println!("Loaded config: {:?}", config);
        }
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            std::process::exit(1);
        }
    }

    println!("Hello, world!");
}
