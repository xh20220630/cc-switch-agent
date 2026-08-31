use std::collections::HashMap;
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

use super::models::RemoteTargetValidationError;

/// 远程目标密码的加密凭据存储。
///
/// Windows 上使用 DPAPI(CryptProtectData/CryptUnprotectData)按当前用户加密，
/// 加密后的密文以 base64 形式保存在 ~/.cc-switch/remote-credentials.json；
/// 密码明文永不写入 remote-targets.json。非 Windows 平台暂不提供凭据保存。
#[derive(Debug, Clone)]
pub struct RemoteCredentialStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CredentialDocument {
    #[serde(default)]
    pub entries: HashMap<String, String>,
}

impl RemoteCredentialStore {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> Self {
        Self::at(
            crate::config::get_home_dir()
                .join(".cc-switch")
                .join("remote-credentials.json"),
        )
    }

    pub fn has(&self, target_id: &str) -> bool {
        self.load()
            .map(|document| document.entries.contains_key(target_id))
            .unwrap_or(false)
    }

    pub fn get(&self, target_id: &str) -> Result<Option<String>, CredentialError> {
        let document = self.load()?;
        let Some(cipher_text) = document.entries.get(target_id) else {
            return Ok(None);
        };
        let encrypted = BASE64
            .decode(cipher_text)
            .map_err(|error| CredentialError::Corrupted(error.to_string()))?;
        let plain = decrypt(&encrypted)?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|error| CredentialError::Corrupted(error.to_string()))
    }

    pub fn set(&self, target_id: &str, password: &str) -> Result<(), CredentialError> {
        let encrypted = encrypt(password.as_bytes())?;
        let mut document = self.load()?;
        document
            .entries
            .insert(target_id.to_string(), BASE64.encode(encrypted));
        self.save(&document)
    }

    pub fn delete(&self, target_id: &str) -> Result<bool, CredentialError> {
        let mut document = self.load()?;
        let removed = document.entries.remove(target_id).is_some();
        if removed {
            self.save(&document)?;
        }
        Ok(removed)
    }

    fn load(&self) -> Result<CredentialDocument, CredentialError> {
        if !self.path.exists() {
            return Ok(CredentialDocument::default());
        }
        let bytes = std::fs::read(&self.path)?;
        if bytes.is_empty() {
            return Ok(CredentialDocument::default());
        }
        serde_json::from_slice(&bytes).map_err(|error| {
            CredentialError::Corrupted(format!("{}: {error}", self.path.display()))
        })
    }

    fn save(&self, document: &CredentialDocument) -> Result<(), CredentialError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp_path = self.path.with_extension(format!(
            "{}.tmp",
            self.path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("json")
        ));
        let bytes = serde_json::to_vec_pretty(document)?;
        std::fs::write(&temp_path, bytes)?;
        if let Err(error) = std::fs::rename(&temp_path, &self.path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(CredentialError::Io(error));
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    use super::CredentialError;

    pub fn encrypt(plain: &[u8]) -> Result<Vec<u8>, CredentialError> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(CredentialError::Platform(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let result =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
        unsafe {
            LocalFree(output.pbData.cast());
        }
        Ok(result)
    }

    pub fn decrypt(encrypted: &[u8]) -> Result<Vec<u8>, CredentialError> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: encrypted.len() as u32,
            pbData: encrypted.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(CredentialError::Platform(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let result =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
        unsafe {
            LocalFree(output.pbData.cast());
        }
        Ok(result)
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::CredentialError;

    pub fn encrypt(_plain: &[u8]) -> Result<Vec<u8>, CredentialError> {
        Err(CredentialError::UnsupportedPlatform)
    }

    pub fn decrypt(_encrypted: &[u8]) -> Result<Vec<u8>, CredentialError> {
        Err(CredentialError::UnsupportedPlatform)
    }
}

fn encrypt(plain: &[u8]) -> Result<Vec<u8>, CredentialError> {
    platform::encrypt(plain)
}

fn decrypt(encrypted: &[u8]) -> Result<Vec<u8>, CredentialError> {
    platform::decrypt(encrypted)
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("凭据文件读写失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("凭据文件损坏: {0}")]
    Corrupted(String),
    #[error("凭据文件序列化失败: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("系统凭据加密失败: {0}")]
    Platform(String),
    #[error("当前平台不支持保存密码")]
    UnsupportedPlatform,
    #[error(transparent)]
    Validation(#[from] RemoteTargetValidationError),
}

#[cfg(test)]
mod credential_store_tests {
    use super::*;

    // DPAPI 仅 Windows 支持；非 Windows encrypt/decrypt 返回 UnsupportedPlatform，
    // 因此加解密往返只在 Windows runner 上验证。
    #[test]
    #[cfg(target_os = "windows")]
    fn round_trips_password_per_target() {
        let temp = tempfile::tempdir().expect("创建凭据 fixture");
        let store = RemoteCredentialStore::at(temp.keep().join("credentials.json"));
        store.set("target-a", "p@ss w0rd!").expect("保存密码");
        store.set("target-b", "another").expect("保存密码");
        assert!(store.has("target-a"));
        assert_eq!(
            store.get("target-a").expect("读取密码").as_deref(),
            Some("p@ss w0rd!")
        );
        assert_eq!(
            store.get("target-b").expect("读取密码").as_deref(),
            Some("another")
        );
        assert!(store.delete("target-a").expect("删除密码"));
        assert!(!store.has("target-a"));
        assert!(store.has("target-b"));
    }

    #[test]
    fn missing_target_returns_none() {
        let temp = tempfile::tempdir().expect("创建凭据 fixture");
        let store = RemoteCredentialStore::at(temp.keep().join("credentials.json"));
        assert!(!store.has("nope"));
        assert_eq!(store.get("nope").expect("读取缺失密码"), None);
    }
}
