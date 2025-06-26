use derive_builder::UninitializedFieldError;
use eyre::Report;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigBuildError {
    #[error("Configuration missing or invalid `{0}`")]
    UninitializedField(&'static str),
    #[error("Configuration error `{0}`")]
    Other(String),
}

impl From<UninitializedFieldError> for ConfigBuildError {
    fn from(err: UninitializedFieldError) -> Self {
        ConfigBuildError::UninitializedField(err.field_name())
    }
}

impl From<Report> for ConfigBuildError {
    fn from(report : Report) -> Self {
        ConfigBuildError::Other(format!("{:?}", report))
    }
}
