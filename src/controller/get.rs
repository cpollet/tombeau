use crate::io::load_shrine_file;
use crate::utils::read_password;
use crate::Error;
use secrecy::Secret;
use std::io::{stdout, Write};
use std::path::PathBuf;

pub fn get(folder: PathBuf, password: Option<Secret<String>>, key: &String) -> Result<(), Error> {
    let shrine_file = load_shrine_file(&folder).map_err(Error::ReadFile)?;

    let password = password.unwrap_or_else(|| read_password(&shrine_file));

    let shrine = shrine_file
        .unwrap(&password)
        .map_err(|e| Error::InvalidFile(e.to_string()))?;

    let secret = shrine
        .get(key.as_ref())
        .ok_or(Error::KeyNotFound(key.to_string()))?;

    let _ = stdout().write_all(secret.expose_secret_as_bytes());

    Ok(())
}
