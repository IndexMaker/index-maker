use serde::{Deserialize, Serialize};

use crate::solver_state::SolverState;

#[derive(Serialize, Deserialize)]
pub struct SolverInput {
    pub state: SolverState,
}