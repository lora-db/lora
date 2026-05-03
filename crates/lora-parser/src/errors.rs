use lora_ast::Span;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("parse error: {message} at {span:?}")]
    Message { message: String, span: Span },
}

impl ParseError {
    pub fn new(message: impl Into<String>, start: usize, end: usize) -> Self {
        Self::Message {
            message: message.into(),
            span: Span::new(start, end),
        }
    }
}
