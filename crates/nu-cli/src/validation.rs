use nu_parser::parse;
use nu_protocol::{
    ParseError,
    engine::{EngineState, StateWorkingSet},
};
use reedline::{ValidationResult, Validator};
use std::{io::Write, sync::Arc};

pub struct NuValidator {
    pub engine_state: Arc<EngineState>,
}

impl Validator for NuValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        let mut working_set = StateWorkingSet::new(&self.engine_state);
        parse(&mut working_set, None, line.as_bytes(), false);

        if matches!(
            working_set.parse_errors.first(),
            Some(ParseError::UnexpectedEof(..))
        ) {
            ValidationResult::Incomplete
        } else {
            // Hide cursor before reedline prints the newline on submit
            let _ = std::io::stdout().write_all(b"\x1b[?25l");
            let _ = std::io::stdout().flush();
            ValidationResult::Complete
        }
    }
}
