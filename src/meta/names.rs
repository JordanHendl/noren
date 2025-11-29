use std::fmt;

pub const DEVICE_NAME_CAPACITY: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceName {
    pub bytes: [u8; DEVICE_NAME_CAPACITY],
}

impl DeviceName {
    /// Creates an empty device-friendly name filled with null bytes.
    pub fn new() -> Self {
        Self {
            bytes: [0; DEVICE_NAME_CAPACITY],
        }
    }

    /// Converts a UTF-8 string into a fixed-size device name, truncating if necessary.
    pub fn from_str(name: &str) -> Self {
        let mut bytes = [0u8; DEVICE_NAME_CAPACITY];
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(DEVICE_NAME_CAPACITY.saturating_sub(1));
        bytes[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        bytes[copy_len] = b'\0';
        Self { bytes }
    }

    /// Converts the fixed-size name back into a trimmed `String`.
    pub fn to_string(&self) -> String {
        let nul_pos = self
            .bytes
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.bytes.len());
        String::from_utf8_lossy(&self.bytes[..nul_pos]).into_owned()
    }
}

impl Default for DeviceName {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&str> for DeviceName {
    fn from(value: &str) -> Self {
        Self::from_str(value)
    }
}

impl From<String> for DeviceName {
    fn from(value: String) -> Self {
        Self::from_str(&value)
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string())
    }
}
