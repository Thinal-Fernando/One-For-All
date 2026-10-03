//! `ofa setup --path`: puts the folder ofa.exe is in on your user PATH, so
//! `ofa` works in any new terminal. The installer runs it; `--uninstall`
//! takes the folder off again. Only your own PATH (HKCU\Environment) is
//! touched, never the system one, and only that one folder is added or
//! removed.

use std::path::Path;

use windows::core::w;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

/// Adds `dir` to the user PATH, or removes it. Returns whether PATH changed.
pub fn run(dir: &Path, uninstall: bool) -> Result<bool, String> {
    let dir = dir
        .to_str()
        .ok_or("the folder of ofa.exe isn't valid text")?;
    let key = Key::open()?;
    let (old, kind) = key.read()?;
    let new = if uninstall {
        without_dir(&old, dir)
    } else {
        with_dir(&old, dir)
    };
    if new == old {
        return Ok(false);
    }
    key.write(&new, kind)?;
    // Tell Explorer, so terminals opened from now on see the new PATH.
    // SAFETY: a broadcast with a static string; the call only reads it.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
    Ok(true)
}

/// Whether two PATH entries name the same folder.
fn same_dir(a: &str, b: &str) -> bool {
    let trim = |s: &str| s.trim().trim_end_matches('\\').to_owned();
    trim(a).eq_ignore_ascii_case(&trim(b))
}

/// `path` with `dir` at the end, unless it is already there.
fn with_dir(path: &str, dir: &str) -> String {
    if path.split(';').any(|entry| same_dir(entry, dir)) {
        return path.to_owned();
    }
    let path = path.trim_end_matches(';');
    if path.is_empty() {
        dir.to_owned()
    } else {
        format!("{path};{dir}")
    }
}

/// `path` without any entry for `dir`. Everything else stays as it was.
fn without_dir(path: &str, dir: &str) -> String {
    if !path.split(';').any(|entry| same_dir(entry, dir)) {
        return path.to_owned();
    }
    path.split(';')
        .filter(|entry| !same_dir(entry, dir))
        .collect::<Vec<_>>()
        .join(";")
}

/// `HKEY_CURRENT_USER\Environment`, open for reading and writing.
struct Key(HKEY);

impl Key {
    fn open() -> Result<Self, String> {
        let mut key = HKEY::default();
        // SAFETY: the out-pointer is a local the call fills in.
        unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                w!("Environment"),
                None,
                KEY_READ | KEY_WRITE,
                &mut key,
            )
        }
        .ok()
        .map_err(|err| format!("couldn't open your environment settings: {err}"))?;
        Ok(Self(key))
    }

    /// The user PATH and its registry type. A missing PATH reads as empty.
    fn read(&self) -> Result<(String, REG_VALUE_TYPE), String> {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: first call only asks for the size; pointers are locals.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                w!("Path"),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        status
            .ok()
            .map_err(|err| format!("couldn't read your PATH: {err}"))?;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err("your PATH isn't stored as text, so OFA left it alone".into());
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: `buf` holds `size` bytes, as the call was told.
        unsafe {
            RegQueryValueExW(
                self.0,
                w!("Path"),
                None,
                Some(&mut kind),
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        }
        .ok()
        .map_err(|err| format!("couldn't read your PATH: {err}"))?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Ok((String::from_utf16_lossy(&buf[..len]), kind))
    }

    fn write(&self, path: &str, kind: REG_VALUE_TYPE) -> Result<(), String> {
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        // SAFETY: the bytes are a live, NUL-terminated UTF-16 buffer.
        let bytes =
            unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
        // SAFETY: `self.0` is open for writing and `bytes` outlives the call.
        unsafe { RegSetValueExW(self.0, w!("Path"), None, kind, Some(bytes)) }
            .ok()
            .map_err(|err| format!("couldn't change your PATH: {err}"))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `open` and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\me\AppData\Local\OFA";

    #[test]
    fn adds_once_at_the_end() {
        let path = r"C:\tools;%USERPROFILE%\bin";
        let added = with_dir(path, DIR);
        assert_eq!(added, format!(r"C:\tools;%USERPROFILE%\bin;{DIR}"));
        assert_eq!(with_dir(&added, DIR), added);
        assert_eq!(with_dir(&format!("{path};"), DIR), added);
        assert_eq!(with_dir("", DIR), DIR);
    }

    #[test]
    fn spots_the_folder_however_it_is_written() {
        let path = r"C:\tools;c:\users\me\appdata\local\ofa\";
        assert_eq!(with_dir(path, DIR), path);
        assert_eq!(without_dir(path, DIR), r"C:\tools");
    }

    #[test]
    fn removes_only_that_folder() {
        let path = format!(r"C:\tools;{DIR};C:\Users\me\AppData\Local\OFA-other;C:\more");
        assert_eq!(
            without_dir(&path, DIR),
            r"C:\tools;C:\Users\me\AppData\Local\OFA-other;C:\more"
        );
        assert_eq!(without_dir(r"C:\tools", DIR), r"C:\tools");
    }
}
