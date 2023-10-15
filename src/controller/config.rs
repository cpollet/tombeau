use crate::git::Repository;

use crate::Error;
use rpassword::prompt_password;

use crate::shrine::{OpenShrine, QueryOpen};
use crate::values::secret::Mode;
use std::io::{stdout, Write};
use std::path::{Path, PathBuf};

pub fn set<P>(
    mut shrine: OpenShrine,
    key: &str,
    value: Option<String>,
    path: P,
) -> Result<(), Error>
where
    P: AsRef<Path> + Clone,
    PathBuf: From<P>,
{
    let value = value.unwrap_or_else(|| prompt_password("Value: ").unwrap());

    shrine.set(&format!(".{key}"), value.as_bytes(), Mode::Text)?;

    let mut repo_path = PathBuf::from(path.clone());
    repo_path.pop();

    let repository = Repository::new(repo_path, &shrine);

    shrine.close()?.write_file(path)?;

    if let Some(repository) = repository {
        if repository.commit_auto() {
            repository
                .open()
                .and_then(|r| r.create_commit("Update shrine"))?;
        }
    }

    Ok(())
}

pub fn get(shrine: &OpenShrine, key: &str) -> Result<(), Error> {
    let secret = shrine.get(key);
    let _ = stdout().write_all(secret.unwrap().value().expose_secret_as_bytes());
    Ok(())
}
