use std::env;

use alloy::{
    providers::{DynProvider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use clap::Parser;
use eyre::Context;
use otc_custody::{
    custody_authority::CustodyAuthority,
    custody_client::CustodyClientMethods,
    index::{
        index_deployment::IndexDeployment,
        index_deployment_serde::{
            IndexDeployerData, IndexDeploymentBuilderData, IndexDeploymentData, IndexInstanceData,
            IndexMakerData,
        },
    },
};
use symm_core::{
    core::{
        json_file_async::{read_from_json_file_async, write_json_to_file_async},
        logging::log_init,
    },
    init_log,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[arg(long, short)]
    file: String,

    #[arg(long, short)]
    output_file: String,

    #[arg(long)]
    rpc_url: Option<String>,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    init_log!();

    tracing::info!("--==| Index Deployer |==--");

    let signer = env::var("INDEX_MAKER_PRIVATE_KEY")
        .expect("INDEX_MAKER_PRIVATE_KEY environment variable must be defined")
        .parse::<PrivateKeySigner>()
        .context("Failed to parse private key")?;

    let index_operator_addres = signer.address();

    let index_operator = CustodyAuthority::new(|| {
        env::var("CUSTODY_AUTHORITY_PRIVATE_KEY")
            .expect("CUSTODY_AUTHORITY_PRIVATE_KEY environment variable must be defined")
    });

    let cli = Cli::parse();

    let rpc_url = cli
        .rpc_url
        .unwrap_or_else(|| String::from("http://127.0.0.1:8545"));

    tracing::info!("* Loading Index Deployer json file: {}", cli.file);

    let deployment_builder_data = read_from_json_file_async::<IndexDeployerData>(&cli.file).await?;
    let builder_data = &deployment_builder_data.deployment_builder_data;

    tracing::info!(
        "✅ Index deployer config ok: chain: {}, custody: {}, factory: {}, trade: {}, withdraw: {}",
        builder_data.chain_id,
        builder_data.custody_address,
        builder_data.index_factory_address,
        builder_data.trade_route,
        builder_data.withdraw_route
    );

    let provider = ProviderBuilder::new()
        .wallet(signer)
        .connect(&rpc_url)
        .await?;

    let mut index_maker_data = IndexMakerData {
        deployment_builder_data: builder_data.clone(),
        indexes: Vec::new(),
    };

    for index_deploy_data in deployment_builder_data.indexes {
        tracing::info!(
            "Deploying index: {} {}",
            index_deploy_data.symbol,
            index_deploy_data.name
        );

        let builder = IndexDeployment::builder_for(
            index_operator.clone(),
            builder_data.chain_id,
            builder_data.index_factory_address,
            builder_data.custody_address,
            builder_data.trade_route,
            builder_data.withdraw_route,
        );

        let deployment = builder
            .build(index_deploy_data)
            .context("Failed to build")?;

        let deployment_data = deployment.get_deploy_data_serde();

        let index = deployment
            .deploy_from(&provider, index_operator_addres)
            .await
            .context("Failed to deploy")?;

        let index_instance_data = IndexInstanceData {
            index_address: *index.get_index_address(),
            deployment_data,
        };

        tracing::info!(
            "✅ Index deployed ok: {} deployed at {}",
            index_instance_data.deployment_data.index_deploy_data.symbol,
            index_instance_data.index_address
        );

        index_maker_data.indexes.push(index_instance_data);
    }

    write_json_to_file_async(&cli.output_file, &index_maker_data)
        .await
        .context("Failed to write Index Maker data to json file")?;

    verify_index_maker_json(index_operator, &rpc_url, &cli.output_file)
        .await
        .context("Failed to verify Index Maker json file data)")?;

    Ok(())
}

async fn verify_index_maker_json(
    index_operator: CustodyAuthority,
    rpc_url: &str,
    path: &str,
) -> eyre::Result<()> {
    tracing::info!("* Verifying Index Maker json file: {}", path);

    let index_maker_data = read_from_json_file_async::<IndexMakerData>(path).await?;
    let builder_data = &index_maker_data.deployment_builder_data;

    tracing::info!(
        "✅ Index maker config ok: chain: {}, custody: {}, factory: {}, trade: {}, withdraw: {}",
        builder_data.chain_id,
        builder_data.custody_address,
        builder_data.index_factory_address,
        builder_data.trade_route,
        builder_data.withdraw_route
    );

    let provider = ProviderBuilder::new().connect(&rpc_url).await?;

    for index_instance_data in index_maker_data.indexes {
        let index_deploy_data = &index_instance_data.deployment_data.index_deploy_data;

        tracing::info!(
            "Verifying index: {} {}",
            index_deploy_data.symbol,
            index_deploy_data.name
        );

        let deployment = IndexDeployment::new_from_deploy_data_serde(
            index_operator.clone(),
            index_instance_data.deployment_data,
        );

        let index = deployment.into_index_at(index_instance_data.index_address);

        match index
            .get_custody_owner(&DynProvider::new(provider.clone()))
            .await
        {
            Ok(owner) => tracing::info!(
                "✅ Index verified ok: {} deployed at {} with owner {}",
                index.get_symbol(),
                index.get_index_address(),
                owner
            ),
            Err(err) => tracing::warn!(
                "Index {} deployed at {} failed with error: {}",
                index.get_symbol(),
                index.get_index_address(),
                err
            ),
        };
    }

    Ok(())
}
