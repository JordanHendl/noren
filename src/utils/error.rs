#[derive(Debug)]
pub enum RdbErr {
    Io(),
    BadHeader,
    TooSmall,
}

impl From<std::io::Error> for RdbErr {
    fn from(value: std::io::Error) -> Self {
        return RdbErr::Io();
    }
}

#[derive(Debug)]
pub enum NorenError {
    Unknown(),
    LookupFailure(),
    UploadFailure(),
    DataFailure(),
    JSONError(serde_json::Error),
    IOFailure(std::io::Error),
    RDBFileError(RdbErr),
}

impl std::fmt::Display for NorenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NorenError::Unknown() => todo!(),
            NorenError::LookupFailure() => todo!(),
            NorenError::UploadFailure() => todo!(),
            NorenError::DataFailure() => todo!(),
            NorenError::RDBFileError(rdb_err) => todo!(),
            NorenError::IOFailure(error) => todo!(),
            NorenError::JSONError(error) => todo!(),
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
