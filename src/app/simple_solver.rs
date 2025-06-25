use std::sync::Arc;

use crate::solver::solvers::simple_solver::SimpleSolver;

use super::config::ConfigBuildError;
use derive_builder::Builder;
use eyre::Result;
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
    pub(crate) simple_solver: Option<Arc<SimpleSolver>>,
}

impl SimpleSolverConfig {
    #[must_use]
    pub fn builder() -> SimpleSolverConfigBuilder {
        SimpleSolverConfigBuilder::default()
    }
}

impl SimpleSolverConfigBuilder {
    pub fn build(self) -> Result<SimpleSolverConfig, ConfigBuildError> {
        let mut config = self.try_build()?;

        let simple_solver = Arc::new(SimpleSolver::new(
            config.price_threshold,
            config.fee_factor,
            config.max_order_volley_size,
            config.max_volley_size,
        ));

        config.simple_solver.replace(simple_solver);

        Ok(config)
    }
}
