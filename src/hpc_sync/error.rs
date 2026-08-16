use std::fmt::{Display, Formatter};

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug)]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
    pub suggested_fix: String,
    pub exit_code: i32,
}

impl AppError {
    pub fn new(
        code: &'static str,
        message: impl Into<String>,
        retryable: bool,
        suggested_fix: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            suggested_fix: suggested_fix.into(),
            exit_code: 2,
        }
    }

    pub fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = exit_code;
        self
    }
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}
