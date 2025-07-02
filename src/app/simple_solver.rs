use std::sync::Arc;

use crate::{
    app::solver::SolverStrategyConfig,
    solver::{solver::SolverStrategy, solvers::simple_solver::SimpleSolver},
};

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::{OptionExt, Result};
use symm_core::core::bits::Amount;

#[derive(Clone, Builder)]
#[builder(
    pattern = "owned",
    build_fn(name = "try_build", error = "ConfigBuildError")
)]
pub struct SimpleSolverConfig {
    #[builder(setter(into, strip_option), default)]
    pub price_threshold: Amount,

    #[builder(setter(into, strip_option), default)]
    pub fee_factor: Amount,

    #[builder(setter(into, strip_option), default)]
    pub max_order_volley_size: Amount,

    #[builder(setter(into, strip_option), default)]
    pub max_volley_size: Amount,

    #[builder(setter(skip))]
    simple_solver: Option<Arc<SimpleSolver>>,
}

impl SimpleSolverConfig {
    #[must_use]
    pub fn builder() -> SimpleSolverConfigBuilder {
        SimpleSolverConfigBuilder::default()
    }

    pub fn expect_simple_solver_cloned(&self) -> Arc<SimpleSolver> {
        self.simple_solver
            .clone()
            .ok_or(())
            .expect("Failed to get simple solver")
    }

    pub fn try_get_simple_solver_cloned(&self) -> Result<Arc<SimpleSolver>> {
        self.simple_solver
            .clone()
            .ok_or_eyre("Failed to get simple solver")
    }
}

impl SolverStrategyConfig for SimpleSolverConfig {
    fn expect_solver_strategy_cloned(&self) -> Arc<dyn SolverStrategy + Send + Sync> {
        self.expect_simple_solver_cloned()
    }

    fn try_get_solver_strategy_cloned(&self) -> Result<Arc<dyn SolverStrategy + Send + Sync>> {
        self.try_get_simple_solver_cloned()
            .map(|x| x as Arc<dyn SolverStrategy + Send + Sync>)
    }
}

impl SimpleSolverConfigBuilder {
    pub fn build_arc(self) -> Result<Arc<SimpleSolverConfig>, ConfigBuildError> {
        let mut config = self.try_build()?;

        let simple_solver = Arc::new(SimpleSolver::new(
            config.price_threshold,
            config.fee_factor,
            config.max_order_volley_size,
            config.max_volley_size,
        ));

        config.simple_solver.replace(simple_solver);

        Ok(Arc::new(config))
    }
}
