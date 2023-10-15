use crate::shrine::encryption::EncryptionAlgorithm;
use crate::shrine::holder::Holder;
use crate::shrine::metadata::Metadata;
use crate::shrine::serialization::SerializationFormat;
use crate::shrine::{OpenShrine, QueryClosed, QueryOpen, VERSION};
use crate::values::password::ShrinePassword;
use crate::values::secret::{Mode, Secret};
use crate::Error;
use borsh::{BorshDeserialize, BorshSerialize};
use std::fmt::{Debug, Formatter};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

// todo move open and closed directly to LocalShrine

pub type Secrets = Holder<Secret>;

pub struct Open {
    secrets: Secrets,
}

#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct Closed(Vec<u8>);

impl Debug for Closed {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Closed(..)")
    }
}

pub struct Password(String);

#[derive(Clone, Debug)]
pub struct NoPassword;

#[derive(Debug)]
pub struct Aes<P = Password> {
    password: P,
}

impl<P> Clone for Aes<P>
where
    P: Clone,
{
    fn clone(&self) -> Self {
        Self {
            password: self.password.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Clear;

#[derive(Default)]
pub struct Unknown;

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct LocalShrine<S = Open, E = Aes<Password>> {
    /// Always "shrine".
    magic_number: [u8; 6],
    metadata: Metadata,
    payload: S,
    #[borsh(skip)]
    encryption: E,
}

impl<T, U> QueryClosed for LocalShrine<T, U> {
    fn uuid(&self) -> Uuid {
        self.metadata.uuid()
    }

    fn version(&self) -> u8 {
        self.metadata.version()
    }

    fn serialization_format(&self) -> SerializationFormat {
        self.metadata.serialization_format()
    }

    fn encryption_algorithm(&self) -> EncryptionAlgorithm {
        self.metadata.encryption_algorithm()
    }
}

impl<T> LocalShrine<Closed, T> {
    fn try_to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut buffer = Vec::new();
        self.write(&mut buffer)?;
        Ok(buffer)
    }

    fn write<W>(&self, writer: &mut W) -> Result<(), Error>
    where
        W: Write,
    {
        self.serialize(writer).map_err(Error::IoWrite)
    }

    pub fn write_file<P>(&self, path: P) -> Result<(), Error>
    where
        P: AsRef<Path>,
    {
        let file = PathBuf::from(path.as_ref().as_os_str());

        let bytes = self.try_to_bytes()?;

        File::create(file)
            .map_err(Error::IoWrite)?
            .write_all(&bytes)
            .map_err(Error::IoWrite)?;

        Ok(())
    }
}

impl<T> Clone for LocalShrine<Closed, T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            magic_number: self.magic_number,
            metadata: self.metadata.clone(),
            payload: self.payload.clone(),
            encryption: self.encryption.clone(),
        }
    }
}

impl LocalShrine<Closed, Clear> {
    pub fn open(self) -> Result<LocalShrine<Open, Clear>, Error> {
        let secrets = self
            .metadata
            .serialization_format()
            .serializer()
            .deserialize(&self.payload.0)?;

        Ok(LocalShrine {
            magic_number: self.magic_number,
            metadata: self.metadata,
            payload: Open { secrets },
            encryption: Clear,
        })
    }
}

impl LocalShrine<Closed, Aes<NoPassword>> {
    // todo change password to ShrinePassword
    pub fn open(self, password: String) -> Result<LocalShrine<Open, Aes<Password>>, Error> {
        let clear_bytes = self
            .metadata
            .encryption_algorithm()
            .encryptor(&ShrinePassword::from(password.clone()), None)
            .decrypt(&self.payload.0)?;

        let secrets = self
            .metadata
            .serialization_format()
            .serializer()
            .deserialize(&clear_bytes)?;

        Ok(LocalShrine {
            magic_number: self.magic_number,
            metadata: self.metadata,
            payload: Open { secrets },
            encryption: Aes {
                password: Password(password),
            },
        })
    }
}

impl<T> LocalShrine<Open, T> {
    pub fn with_serialization_format(&mut self, format: SerializationFormat) {
        self.metadata = match self.metadata {
            Metadata::V0 {
                uuid,
                encryption_algorithm,
                ..
            } => Metadata::V0 {
                uuid,
                encryption_algorithm,
                serialization_format: format,
            },
        };
    }
}

impl<T> QueryOpen for LocalShrine<Open, T> {
    type Error = Error;

    fn set(&mut self, key: &str, value: &[u8], mode: Mode) -> Result<(), Self::Error> {
        if let Some(key) = key.strip_prefix('.') {
            return self
                .payload
                .secrets
                .set_private(key, Secret::new(value.into(), mode));
        }

        match self.payload.secrets.get_mut(key) {
            Ok(secret) => {
                secret.with_data(value.into(), mode);
                Ok(())
            }
            Err(Error::KeyNotFound(_)) => self
                .payload
                .secrets
                .set(key, Secret::new(value.into(), mode)),
            Err(e) => Err(e),
        }
    }

    fn get(&self, key: &str) -> Result<&Secret, Self::Error> {
        if let Some(key) = key.strip_prefix('.') {
            return self.payload.secrets.get_private(key);
        }
        self.payload.secrets.get(key)
    }

    fn rm(&mut self, key: &str) -> bool {
        self.payload.secrets.remove(key)
    }

    fn mv(self, other: &mut OpenShrine) {
        match other {
            OpenShrine::LocalClear(s) => s.payload = self.payload,
            OpenShrine::LocalAes(s) => s.payload = self.payload,
            OpenShrine::Remote(_) => {
                unimplemented!("Moving a local shrine to remote one is not supported")
            }
        }
    }

    fn keys(&self) -> Vec<String> {
        self.payload.secrets.keys()
    }

    fn keys_private(&self) -> Vec<String> {
        self.payload.secrets.keys_private()
    }
}

impl<T> LocalShrine<Open, Aes<T>> {
    pub fn into_clear(self) -> LocalShrine<Open, Clear> {
        LocalShrine {
            magic_number: self.magic_number,
            metadata: match self.metadata {
                Metadata::V0 {
                    uuid,
                    serialization_format,
                    ..
                } => Metadata::V0 {
                    uuid,
                    encryption_algorithm: EncryptionAlgorithm::Plain,
                    serialization_format,
                },
            },
            payload: self.payload,
            encryption: Clear,
        }
    }

    pub fn set_password(self, password: String) -> LocalShrine<Open, Aes<Password>> {
        LocalShrine {
            magic_number: self.magic_number,
            metadata: self.metadata,
            payload: self.payload,
            encryption: Aes {
                password: Password(password),
            },
        }
    }
}

impl LocalShrine<Open, Aes<NoPassword>> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn close(self, password: String) -> Result<LocalShrine<Closed, Aes<NoPassword>>, Error> {
        self.set_password(password).close()
    }
}

impl Default for LocalShrine<Open, Aes<NoPassword>> {
    fn default() -> Self {
        Self {
            magic_number: [b's', b'h', b'r', b'i', b'n', b'e'],
            metadata: Metadata::V0 {
                uuid: Uuid::new_v4().as_u128(),
                encryption_algorithm: EncryptionAlgorithm::Aes,
                serialization_format: Default::default(),
            },
            payload: Open {
                secrets: Holder::new(),
            },
            encryption: Aes {
                password: NoPassword,
            },
        }
    }
}

impl LocalShrine<Open, Aes<Password>> {
    pub fn close(self) -> Result<LocalShrine<Closed, Aes<NoPassword>>, Error> {
        let clear_bytes = self
            .metadata
            .serialization_format()
            .serializer()
            .serialize(&self.payload.secrets)?;

        let password = ShrinePassword::from(self.encryption.password.0);

        let cipher_bytes = self
            .metadata
            .encryption_algorithm()
            .encryptor(&password, None)
            .encrypt(&clear_bytes)?;

        Ok(LocalShrine {
            magic_number: self.magic_number,
            metadata: self.metadata,
            payload: Closed(cipher_bytes),
            encryption: Aes {
                password: NoPassword,
            },
        })
    }
}

impl LocalShrine<Open, Clear> {
    pub fn into_aes(self) -> LocalShrine<Open, Aes<NoPassword>> {
        LocalShrine {
            magic_number: self.magic_number,
            metadata: match self.metadata {
                Metadata::V0 {
                    uuid,
                    serialization_format,
                    ..
                } => Metadata::V0 {
                    uuid,
                    encryption_algorithm: EncryptionAlgorithm::Aes,
                    serialization_format,
                },
            },
            payload: self.payload,
            encryption: Aes {
                password: NoPassword,
            },
        }
    }

    pub fn into_aes_with_password(self, password: String) -> LocalShrine<Open, Aes<Password>> {
        let shrine = self.into_aes();
        LocalShrine {
            magic_number: shrine.magic_number,
            metadata: shrine.metadata,
            payload: shrine.payload,
            encryption: Aes {
                password: Password(password),
            },
        }
    }

    pub fn close(self) -> Result<LocalShrine<Closed, Clear>, Error> {
        let bytes = self
            .metadata
            .serialization_format()
            .serializer()
            .serialize(&self.payload.secrets)?;

        Ok(LocalShrine {
            magic_number: self.magic_number,
            metadata: self.metadata,
            payload: Closed(bytes),
            encryption: Clear,
        })
    }
}

#[derive(Debug)]
pub enum LoadedShrine {
    Clear(LocalShrine<Closed, Clear>),
    Aes(LocalShrine<Closed, Aes<NoPassword>>),
}

impl LoadedShrine {
    /// Read a shrine from a path.
    pub fn try_from_path<P>(path: P) -> Result<Self, Error>
    where
        P: AsRef<Path>,
    {
        if !path.as_ref().exists() {
            return Err(Error::FileNotFound(path.as_ref().to_path_buf()));
        }

        let bytes = {
            let mut file = File::open(&path).map_err(Error::IoRead)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(Error::IoRead)?;
            bytes
        };

        Self::try_from_bytes(&bytes)
    }

    /// Read a shrine from a byte slice.
    fn try_from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 6 || &bytes[0..6] != "shrine".as_bytes() {
            return Err(Error::Read());
        }

        if bytes[6] > VERSION {
            return Err(Error::UnsupportedVersion(bytes[6]));
        }

        let shrine =
            LocalShrine::<Closed, Unknown>::try_from_slice(bytes).map_err(Error::IoRead)?;

        Ok(match shrine.metadata {
            Metadata::V0 {
                encryption_algorithm,
                ..
            } => match encryption_algorithm {
                EncryptionAlgorithm::Aes => LoadedShrine::Aes(LocalShrine {
                    magic_number: shrine.magic_number,
                    metadata: shrine.metadata,
                    payload: shrine.payload,
                    encryption: Aes {
                        password: NoPassword,
                    },
                }),
                EncryptionAlgorithm::Plain => LoadedShrine::Clear(LocalShrine {
                    magic_number: shrine.magic_number,
                    metadata: shrine.metadata,
                    payload: shrine.payload,
                    encryption: Clear,
                }),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shrine::VERSION;
    use tempfile::tempdir;

    #[test]
    fn local_shrine_uuid() {
        let shrine = LocalShrine::new();
        let uuid = (&shrine.metadata).uuid();
        assert_eq!(shrine.uuid().as_u128(), uuid.as_u128());
    }

    #[test]
    fn local_shrine_version() {
        let shrine = LocalShrine::new();
        assert_eq!(shrine.version(), VERSION);
    }

    #[test]
    fn local_shrine_serialization_format() {
        let shrine = LocalShrine::new();
        assert_eq!(shrine.serialization_format(), SerializationFormat::Bson);
    }

    #[test]
    fn local_shrine_encryption_format() {
        let shrine = LocalShrine::new();
        assert_eq!(shrine.encryption_algorithm(), EncryptionAlgorithm::Aes);

        let shrine = LocalShrine::new().into_clear();
        assert_eq!(shrine.encryption_algorithm(), EncryptionAlgorithm::Plain);
    }

    #[test]
    fn loaded_shrine_uuid() {
        let shrine = LocalShrine::new();
        let uuid = (&shrine.metadata).uuid();
        assert_eq!(shrine.uuid().as_u128(), uuid.as_u128());
    }

    #[test]
    fn loaded_shrine_version() {
        let shrine = LocalShrine::new();
        assert_eq!(shrine.version(), VERSION);
    }

    #[test]
    fn loaded_shrine_serialization_format() {
        let shrine = LocalShrine::new();
        assert_eq!(shrine.serialization_format(), SerializationFormat::Bson);
    }

    #[test]
    fn loaded_shrine_encryption_format() {
        let shrine = LocalShrine::new();
        assert_eq!(shrine.encryption_algorithm(), EncryptionAlgorithm::Aes);

        let shrine = LocalShrine::new().into_clear();
        assert_eq!(shrine.encryption_algorithm(), EncryptionAlgorithm::Plain);
    }

    #[test]
    fn set_get() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();
        let secret = shrine.get("key").unwrap();
        assert_eq!(secret.value().expose_secret_as_bytes(), "value".as_bytes());
        assert_eq!(secret.mode(), Mode::Text);

        shrine.set("key", "bin".as_bytes(), Mode::Binary).unwrap();
        let secret = shrine.get("key").unwrap();
        assert_eq!(secret.value().expose_secret_as_bytes(), "bin".as_bytes());
        assert_eq!(secret.mode(), Mode::Binary);
    }

    #[test]
    fn set_get_private() {
        let mut shrine = LocalShrine::new();

        shrine.set(".key", "value".as_bytes(), Mode::Text).unwrap();
        let secret = shrine.get(".key").unwrap();
        assert_eq!(secret.value().expose_secret_as_bytes(), "value".as_bytes());
        assert_eq!(secret.mode(), Mode::Text);

        shrine.set(".key", "bin".as_bytes(), Mode::Binary).unwrap();
        let secret = shrine.get(".key").unwrap();
        assert_eq!(secret.value().expose_secret_as_bytes(), "bin".as_bytes());
        assert_eq!(secret.mode(), Mode::Binary);
    }

    #[test]
    fn rm() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();
        assert!(shrine.rm("key"));

        let err = shrine.get("key").unwrap_err();
        match err {
            Error::KeyNotFound(k) => {
                assert_eq!(&k, "key")
            }
            e => panic!("Expected Error::KeyNotFound(\"key\"), got {:?}", e),
        }

        assert!(!shrine.rm("key"));
    }

    #[test]
    fn mv() {
        let mut src = LocalShrine::new();
        src.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let mut dst = OpenShrine::LocalClear(LocalShrine::new().into_clear());
        src.mv(&mut dst);

        let secret = dst.get("key").unwrap();
        assert_eq!(secret.value().expose_secret_as_bytes(), "value".as_bytes());
        assert_eq!(secret.mode(), Mode::Text);
    }

    #[test]
    fn keys() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let keys = shrine.keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.get(0), Some(&"key".to_string()))
    }

    #[test]
    fn keys_private() {
        let mut shrine = LocalShrine::new();

        shrine.set(".key", "value".as_bytes(), Mode::Text).unwrap();

        let keys = shrine.keys_private();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.get(0), Some(&"key".to_string()))
    }

    #[test]
    fn clear_close_open() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let shrine = shrine.into_clear();

        let shrine = shrine.close().unwrap();

        let shrine = shrine.open().unwrap();

        assert_eq!(
            shrine.get("key").unwrap().value().expose_secret_as_bytes(),
            "value".as_bytes()
        );
    }

    #[test]
    fn aes_close_open() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let shrine = shrine.close("password".to_string()).unwrap();

        let shrine = shrine.open("password".to_string()).unwrap();

        assert_eq!(
            shrine.get("key").unwrap().value().expose_secret_as_bytes(),
            "value".as_bytes()
        );
    }

    #[test]
    fn aes_close_open_wrong_password() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let shrine = shrine.set_password("password".to_string());

        let shrine = shrine.close().unwrap();

        match shrine.open("wrong".to_string()) {
            Err(Error::CryptoRead) => (),
            _ => panic!("Expected Err(Error::CryptoRead)"),
        }
    }

    #[test]
    fn clear_try_to_bytes_try_from_bytes() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let shrine = shrine.into_clear().close().unwrap();

        let bytes = shrine.try_to_bytes().unwrap();

        let shrine = match LoadedShrine::try_from_bytes(&bytes).unwrap() {
            LoadedShrine::Clear(s) => s.open().unwrap(),
            _ => panic!("Expected clear shrine"),
        };

        assert_eq!(
            shrine.get("key").unwrap().value().expose_secret_as_bytes(),
            "value".as_bytes()
        );
    }

    #[test]
    fn aes_try_to_bytes_try_from_bytes() {
        let mut shrine = LocalShrine::new();

        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();

        let shrine = shrine
            .into_clear()
            .into_aes_with_password("password".to_string())
            .close()
            .unwrap();

        let bytes = shrine.try_to_bytes().unwrap();

        let shrine = match LoadedShrine::try_from_bytes(&bytes).unwrap() {
            LoadedShrine::Aes(s) => s.open("password".to_string()).unwrap(),
            _ => panic!("Expected aes shrine"),
        };

        assert_eq!(
            shrine.get("key").unwrap().value().expose_secret_as_bytes(),
            "value".as_bytes()
        );
    }

    #[test]
    fn write_file_try_from_path() {
        let folder = tempdir().unwrap();
        let mut path = folder.into_path();
        path.push("shrine");

        let mut shrine = LocalShrine::new();
        shrine.set("key", "value".as_bytes(), Mode::Text).unwrap();
        let shrine = shrine.close("password".to_string()).unwrap();
        shrine.write_file(&path).unwrap();

        let shrine = LoadedShrine::try_from_path(&path).unwrap();

        let shrine = match shrine {
            LoadedShrine::Clear(_) => panic!("AES shrine expected"),
            LoadedShrine::Aes(s) => s.open("password".to_string()).unwrap(),
        };

        assert_eq!(
            shrine.get("key").unwrap().value().expose_secret_as_bytes(),
            "value".as_bytes()
        );
    }

    #[test]
    fn invalid_magic_number() {
        let mut bytes = LocalShrine::new()
            .into_clear()
            .close()
            .unwrap()
            .try_to_bytes()
            .unwrap();
        bytes[0] += 1;

        match LoadedShrine::try_from_bytes(&bytes).unwrap_err() {
            Error::Read() => {}
            e => panic!("expected Error::Read, got {:?}", e),
        }
    }

    #[test]
    fn unsupported_version() {
        let mut bytes = LocalShrine::new()
            .into_clear()
            .close()
            .unwrap()
            .try_to_bytes()
            .unwrap();
        bytes[6] += 1;

        match LoadedShrine::try_from_bytes(&bytes).unwrap_err() {
            Error::UnsupportedVersion(v) => {
                assert_eq!(v, 1)
            }
            e => panic!("expected Error::Read, got {:?}", e),
        }
    }
}
