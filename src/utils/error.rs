#[derive(Debug)]
pub enum RdbErr {
    Io(),
    BadHeader,
    TooSmall,
    NameTooLong,
}

impl std::fmt::Display for RdbErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RdbErr::Io() => write!(f, "RDB file I/O error"),
            RdbErr::BadHeader => write!(f, "RDB file has an invalid header"),
            RdbErr::TooSmall => write!(f, "RDB file is too small"),
            RdbErr::NameTooLong => write!(f, "RDB entry name exceeds 63 bytes"),
        }
    }
}

impl From<std::io::Error> for RdbErr {
    fn from(_: std::io::Error) -> Self {
        RdbErr::Io()
    }
}

#[derive(Debug)]
pub enum NorenError {
    Unknown(),
    LookupFailure(),
    UploadFailure(),
    DataFailure(),
    JSONError(serde_json::Error),
    YAMLError(serde_yaml::Error),
    IOFailure(std::io::Error),
    RDBFileError(RdbErr),
}

impl std::fmt::Display for NorenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NorenError::Unknown() => write!(f, "An unknown error occurred."),
            NorenError::LookupFailure() => {
                write!(f, "Failed to locate the requested resource.")
            }
            NorenError::UploadFailure() => write!(f, "Failed to upload data."),
            NorenError::DataFailure() => write!(f, "Data processing failed."),
            NorenError::RDBFileError(rdb_err) => write!(f, "RDB file error: {}", rdb_err),
            NorenError::IOFailure(error) => write!(f, "I/O failure: {}", error),
            NorenError::JSONError(error) => write!(f, "JSON processing error: {}", error),
            NorenError::YAMLError(error) => write!(f, "YAML processing error: {}", error),
        }
    }
}

impl From<RdbErr> for NorenError {
    fn from(value: RdbErr) -> Self {
        NorenError::RDBFileError(value)
    }
}

impl From<serde_json::Error> for NorenError {
    fn from(value: serde_json::Error) -> Self {
        NorenError::JSONError(value)
    }
}

impl From<serde_yaml::Error> for NorenError {
    fn from(value: serde_yaml::Error) -> Self {
        NorenError::YAMLError(value)
    }
}

impl From<std::io::Error> for NorenError {
    fn from(value: std::io::Error) -> Self {
        return NorenError::IOFailure(value);
    }
}

impl std::error::Error for NorenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }

    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }

    fn cause(&self) -> Option<&dyn std::error::Error> {
        self.source()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_human_readable() {
        assert_eq!(
            format!("{}", NorenError::Unknown()),
            "An unknown error occurred."
        );
        assert_eq!(
            format!("{}", NorenError::LookupFailure()),
            "Failed to locate the requested resource."
        );
        assert_eq!(
            format!("{}", NorenError::UploadFailure()),
            "Failed to upload data."
        );
        assert_eq!(
            format!("{}", NorenError::DataFailure()),
            "Data processing failed."
        );
        assert_eq!(
            format!("{}", NorenError::RDBFileError(RdbErr::BadHeader)),
            "RDB file error: RDB file has an invalid header"
        );

        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "disk full");
        assert_eq!(
            format!("{}", NorenError::IOFailure(io_err)),
            "I/O failure: disk full"
        );

        let json_err: serde_json::Error = serde_json::from_str::<serde_json::Value>("not json")
            .expect_err("expected JSON parsing to fail");
        assert_eq!(
            format!("{}", NorenError::JSONError(json_err)),
            "JSON processing error: expected ident at line 1 column 2"
        );

        let yaml_err: serde_yaml::Error =
            serde_yaml::from_str::<serde_yaml::Value>("- not: yaml: :")
                .expect_err("expected YAML parsing to fail");
        assert!(
            format!("{}", NorenError::YAMLError(yaml_err)).starts_with("YAML processing error:")
        );
    }

    #[test]
    fn rdb_error_display_variants() {
        assert_eq!(format!("{}", RdbErr::Io()), "RDB file I/O error");
        assert_eq!(
            format!("{}", RdbErr::BadHeader),
            "RDB file has an invalid header"
        );
        assert_eq!(format!("{}", RdbErr::TooSmall), "RDB file is too small");
        assert_eq!(
            format!("{}", RdbErr::NameTooLong),
            "RDB entry name exceeds 63 bytes"
        );
    }
}
