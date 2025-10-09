use crate::{solver_input::SolverInput, solver_output::SolverOutput};

pub struct SolverService {}

impl SolverService {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn solve(&self, input: SolverInput) -> eyre::Result<SolverOutput> {
        Ok(SolverOutput { state: input.state })
    }
}
