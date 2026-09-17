use anyhow::Result;

const TARGET_PREFIX: &str = "SapiensAgent/provider/";

pub fn store_provider_key(alias: &str, value: &str) -> Result<()> {
    #[cfg(windows)]
    {
        return windows_store_provider_key(alias, value);
    }
    #[cfg(not(windows))]
    {
        let _ = (alias, value);
        anyhow::bail!("armazenamento seguro de credenciais só está disponível no Windows")
    }
}

pub fn read_provider_key(alias: &str) -> Option<String> {
    #[cfg(windows)]
    {
        return windows_read_provider_key(alias);
    }
    #[cfg(not(windows))]
    {
        let _ = alias;
        None
    }
}

fn target(alias: &str) -> String {
    format!("{TARGET_PREFIX}{alias}")
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn windows_store_provider_key(alias: &str, value: &str) -> Result<()> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredWriteW,
    };

    let target = wide(&target(alias));
    let username = wide("sapiens-agent");
    let mut blob = value.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_ptr() as *mut u16,
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: username.as_ptr() as *mut u16,
        ..Default::default()
    };
    let written = unsafe { CredWriteW(&credential, 0) };
    blob.fill(0);
    if written == 0 {
        anyhow::bail!("não foi possível salvar a credencial no Gerenciador de Credenciais")
    }
    Ok(())
}

#[cfg(windows)]
fn windows_read_provider_key(alias: &str) -> Option<String> {
    use std::{ffi::c_void, ptr::null_mut, slice};
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };

    let target = wide(&target(alias));
    let mut credential: *mut CREDENTIALW = null_mut();
    let read = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    if read == 0 || credential.is_null() {
        return None;
    }
    let result = unsafe {
        let credential = &*credential;
        if credential.CredentialBlob.is_null() {
            None
        } else {
            let bytes = slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            String::from_utf8(bytes.to_vec()).ok()
        }
    };
    unsafe { CredFree(credential.cast::<c_void>()) };
    result.filter(|value| !value.trim().is_empty())
}
