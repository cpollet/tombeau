use clap::{Parser, Subcommand, ValueEnum};
use shrine::controller::get::get;
use shrine::controller::init::init;
use shrine::controller::ls::ls;
use shrine::controller::rm::rm;
use shrine::controller::set::set;
use std::env;

use std::path::PathBuf;

use shrine::Error;

use secrecy::Secret;
use shrine::controller::config;
use shrine::controller::convert::convert;
use shrine::controller::dump::dump;
use shrine::controller::import::import;
use shrine::controller::info::{info, Fields};
use shrine::shrine::{EncryptionAlgorithm, Shrine};
use std::process::ExitCode;

#[derive(Clone, Parser)]
#[command(author, version, about, long_about = None, arg_required_else_help = true)]
struct Args {
    /// The password to use; if not provided, will be prompted interactively when needed
    #[arg(short = 'P', long)]
    password: Option<String>,
    /// The folder containing the shrine file; default is `SHRINE_PATH` env variable or `.` if not set
    #[arg(short, long)]
    path: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Clone, Subcommand)]
enum Commands {
    /// Initializes a shrine in the current folder
    Init {
        /// Override any existing shrine
        #[arg(long, short)]
        force: bool,
        /// Encryption algorithm to use
        #[arg(long, short)]
        encryption: Option<EncryptionAlgorithms>,
        /// Initialize a git repository to contain the shrine
        #[arg(long, short)]
        git: bool,
    },
    /// Convert a shrine to a different format and/or password. This always changes the shrine's
    /// UUID
    Convert {
        /// Change the password; if set and no new password is provided, it will be prompted
        #[arg(long, short, default_value = "false")]
        change_password: bool,
        /// The new password to use; if set, implies password change
        #[arg(long, short)]
        new_password: Option<String>,
        /// New encryption algorithm to use (implies password change)
        #[arg(long, short)]
        encryption: Option<EncryptionAlgorithms>,
    },
    /// Get metadata information about the shrine
    Info {
        /// The field to extract
        #[arg(long, short)]
        field: Option<InfoFields>,
    },
    /// Sets a secret key/value pair
    Set {
        /// The secret's key
        key: String,
        /// The secret's value; if not set, will be prompted
        value: Option<String>,
    },
    /// Get a secret's value
    Get {
        /// The secret's key
        key: String,
    },
    /// Lists all secrets keys
    Ls {
        /// Only lists the key matching the provided pattern
        #[arg(value_name = "REGEX")]
        pattern: Option<String>,
    },
    /// Removes secrets stored in keys matching the provided pattern
    Rm {
        /// The secret's key to remove
        #[arg(value_name = "REGEX")]
        key: String,
    },
    /// Imports secret and their values from environment file
    Import {
        /// The file to import
        file: PathBuf,
        /// Prefix keys with value
        #[arg(long, short)]
        prefix: Option<String>,
    },
    /// Dumps the secrets in a `key=value` format
    Dump {
        /// Only dump the key matching the provided pattern
        #[arg(value_name = "REGEX")]
        pattern: Option<String>,
        /// Include configuration keys
        #[arg(long, short, default_value = "false")]
        config: bool,
    },
    /// Configures the shrine
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommands>,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum EncryptionAlgorithms {
    /// No encryption
    None,
    /// AES-GCM-SIV with 256-bits key
    Aes,
}

impl From<EncryptionAlgorithms> for EncryptionAlgorithm {
    fn from(value: EncryptionAlgorithms) -> Self {
        match value {
            EncryptionAlgorithms::None => EncryptionAlgorithm::Plain,
            EncryptionAlgorithms::Aes => EncryptionAlgorithm::Aes,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum InfoFields {
    Version,
    Uuid,
    EncryptionAlgorithm,
    SerializationFormat,
}

impl From<InfoFields> for Fields {
    fn from(value: InfoFields) -> Self {
        match value {
            InfoFields::Version => Fields::Version,
            InfoFields::Uuid => Fields::Uuid,
            InfoFields::EncryptionAlgorithm => Fields::Encryption,
            InfoFields::SerializationFormat => Fields::Serialization,
        }
    }
}

#[derive(Clone, Subcommand)]
#[command(arg_required_else_help = true)]
enum ConfigCommands {
    /// Sets a configuration option
    Set {
        /// The configuration key
        key: String,
        /// The configuration value; if not set, will be prompted
        value: Option<String>,
    },
    /// Get a configuration option's value
    Get {
        /// The configuration key
        key: String,
    },
}

#[allow(unused)]
fn main() -> ExitCode {
    match exec(Args::parse()) {
        Ok(_) => ExitCode::from(0),
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::from(1)
        }
    }
}

fn exec(cli: Args) -> Result<(), Error> {
    let password = cli.password.map(Secret::new);
    let path = cli
        .path
        .unwrap_or_else(|| PathBuf::from(env::var("SHRINE_PATH").unwrap_or(".".to_string())));

    match &cli.command {
        Some(Commands::Init {
            force,
            encryption,
            git,
        }) => init(
            path,
            password,
            *force,
            encryption.map(|algo| algo.into()),
            *git,
        ),
        Some(Commands::Convert {
            change_password,
            new_password,
            encryption,
        }) => convert(
            Shrine::from_path(&path)?,
            path,
            password,
            *change_password,
            new_password.clone().map(Secret::new),
            encryption.map(|algo| algo.into()),
        ),
        Some(Commands::Info { field }) => {
            info(Shrine::from_path(&path)?, path, (*field).map(Fields::from))
        }
        Some(Commands::Set { key, value }) => set(
            Shrine::from_path(&path)?,
            path,
            password,
            key,
            value.as_deref(),
        ),
        Some(Commands::Get { key }) => get(Shrine::from_path(&path)?, password, key),
        Some(Commands::Ls { pattern }) => ls(Shrine::from_path(&path)?, password, pattern.as_ref()),
        Some(Commands::Rm { key }) => rm(Shrine::from_path(&path)?, path, password, key),
        Some(Commands::Import { file, prefix }) => import(
            Shrine::from_path(&path)?,
            path,
            password,
            file,
            prefix.as_deref(),
        ),
        Some(Commands::Dump { pattern, config }) => dump(
            Shrine::from_path(&path)?,
            path,
            password,
            pattern.as_ref(),
            *config,
        ),
        Some(Commands::Config { command }) => match command {
            Some(ConfigCommands::Set { key, value }) => config::set(
                Shrine::from_path(&path)?,
                path,
                password,
                key,
                value.as_deref(),
            ),
            Some(ConfigCommands::Get { key }) => {
                config::get(Shrine::from_path(&path)?, path, password, key)
            }
            _ => panic!(),
        },
        _ => panic!(),
    }
}
