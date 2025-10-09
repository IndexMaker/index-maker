use serde::{Deserialize, Serialize};

use crate::solver_state::SolverState;

#[derive(Serialize, Deserialize)]
pub struct SolverOutput {
    pub state: SolverState,
}
