use crate::git::Repository;
use crate::io::{load_shrine, save_shrine};
use crate::utils::read_password;
use crate::Error;
use rpassword::prompt_password;
use secrecy::Secret;
use std::io::{stdout, Write};
use std::path::PathBuf;

pub fn set(
    path: PathBuf,
    password: Option<Secret<String>>,
    key: &String,
    value: Option<&str>,
) -> Result<(), Error> {
    let shrine = load_shrine(&path).map_err(Error::ReadFile)?;

    let password = password.unwrap_or_else(|| read_password(&shrine));

    let mut shrine = shrine
        .open(&password)
        .map_err(|e| Error::InvalidFile(e.to_string()))?;

    let value = value
        .map(|v| v.to_string())
        .unwrap_or_else(|| prompt_password("Value: ").unwrap());

    shrine.set_private(key.to_string(), value);

    let repository = Repository::new(path.clone(), &shrine);

    // let mut shrine_file = shrine;
    // shrine_file
    //     .wrap(shrine, &password)
    //     .map_err(|e| Error::Update(e.to_string()))?;
    let shrine = shrine
        .close(&password)
        .map_err(|e| Error::Update(e.to_string()))?;

    save_shrine(&path, &shrine)
        .map_err(Error::WriteFile)
        .map(|_| ())?;

    if let Some(repository) = repository {
        if repository.commit_auto() {
            repository
                .open()
                .and_then(|r| r.create_commit("Update shrine"))
                .map_err(Error::Git)?;
        }
    }

    Ok(())
}

pub fn get(path: PathBuf, password: Option<Secret<String>>, key: &String) -> Result<(), Error> {
    let shrine_file = load_shrine(&path).map_err(Error::ReadFile)?;

    let password = password.unwrap_or_else(|| read_password(&shrine_file));

    let shrine = shrine_file
        .open(&password)
        .map_err(|e| Error::InvalidFile(e.to_string()))?;

    let secret = shrine
        .get_private(key.as_ref())
        .ok_or(Error::KeyNotFound(key.to_string()))?;

    let _ = stdout().write_all(secret.as_bytes());

    Ok(())
}
