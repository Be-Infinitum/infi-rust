//! Prepaid: product with a grant and a per_unit price, wallet credit, usage and the debit by the worker.

use beinfi::*;
use std::time::{Duration, Instant};

#[path = "common/mod.rs"]
mod common;
use common::{show, show_json, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    let s = common::uniq();

    step("Declare the prepaid product (billing as code)");
    let product_key = format!("credits-{s}");
    let document = CompanyDocument {
        schema_version: 1,
        products: vec![CompanyProduct {
            key: product_key.clone(),
            name: Some("AI credits".into()),
            r#type: CompanyProductType::Agent,
            pricing_model: CompanyProductPricingModel::Prepaid,
            currency: Some("BRL".into()),
            base_price: Some("0".into()),
            billing_cycle: Some(CompanyProductBillingCycle::Monthly),
            meters: Some(vec![CompanyMeter {
                key: "tokens".into(),
                unit: CompanyMeterUnit::Token,
                aggregation: CompanyMeterAggregation::Sum,
                ..Default::default()
            }]),
            prices: Some(vec![CompanyPrice {
                meter: "tokens".into(),
                model: CompanyPriceModel::PerUnit,
                unit_amount: Some("0.01".into()),
                ..Default::default()
            }]),
            grants: Some(vec![CompanyGrant {
                meter: "tokens".into(),
                amount: "1000".into(),
                on: CompanyGrantOn::Payment,
            }]),
            ..Default::default()
        }],
        webhooks: None,
    };
    let plan = client
        .config()
        .plan(ConfigPlanInput {
            body: ConfigPlanRequest {
                document: document.clone(),
                ..Default::default()
            },
            ..Default::default()
        })
        .await?;
    let applied = client
        .config()
        .apply_with(
            ConfigApplyInput {
                body: ConfigApplyRequest {
                    document,
                    ..Default::default()
                },
                ..Default::default()
            },
            CallOptions::new().if_match(plan.plan_hash),
        )
        .await?;
    show(
        "revision",
        format!("#{} {:?}", applied.revision.number, applied.revision.status),
    );

    let meter = "tokens";
    show("productKey / meter", format!("{product_key} / {meter}"));

    step("Create the customer");
    let external_id = format!("user_{}", common::uniq());
    let customer = client
        .customers()
        .create(CustomersCreateInput {
            body: CustomersCreateRequest {
                product_key: Some(product_key.clone()),
                external_id: external_id.clone(),
                ..Default::default()
            },
        })
        .await?;
    let wallet_id = customer.enrollment_id.clone();
    show("enrollmentId (wallet id)", &wallet_id);

    let balance_of = || {
        let client = client.clone();
        let wallet_id = wallet_id.clone();
        async move {
            let w = client
                .customers()
                .get_wallet(CustomersGetWalletInput {
                    path: CustomersGetWalletPath {
                        enrollment_id: wallet_id,
                    },
                    query: CustomersGetWalletQuery {
                        meter: Some(meter.to_string()),
                    },
                })
                .await?;
            // The response is a oneOf: one balance (with meter) or the list of balances.
            Ok::<String, Error>(match w {
                CustomersGetWalletResponse::WalletBalance(one) => one.balance,
                CustomersGetWalletResponse::WalletBalances(many) => many
                    .balances
                    .into_iter()
                    .find(|b| b.meter == meter)
                    .map_or_else(|| "0".to_string(), |b| b.balance),
            })
        }
    };

    step("Credit 1000 to the wallet");
    let credited = client
        .customers()
        .credit_wallet(CustomersCreditWalletInput {
            path: CustomersCreditWalletPath {
                enrollment_id: wallet_id.clone(),
            },
            body: WalletMutationRequest {
                meter: meter.into(),
                amount: "1000".into(),
                reason: Some("starter pack".into()),
                ..Default::default()
            },
        })
        .await?;
    show_json("wallet", &credited);
    let before = balance_of().await?;
    show("balance before", &before);

    step("Record a consumption of 200");
    let rec = client
        .usage()
        .record(UsageRecordInput {
            body: FacadeUsageEvent {
                product_key: Some(product_key.clone()),
                customer_id: external_id.clone(),
                meter: meter.into(),
                value: Some("200".into()),
                event_id: format!("usage-{}", common::uniq()),
                ..Default::default()
            },
        })
        .await?;
    show_json("event", &rec);

    step("Wait for the worker to debit (reading the balance every 10s, up to 3 min)");
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut after = balance_of().await?;
    while after == before && Instant::now() < deadline {
        common::sleep(Duration::from_secs(10)).await;
        after = balance_of().await?;
        println!("   ... balance now: {after}");
    }
    show("balance before", &before);
    show("balance after", &after);
    if after == before {
        return Err("the balance did not drop in 3 minutes: the worker did not debit the wallet of the prepaid product".into());
    }
    let b: f64 = before.parse().unwrap_or(0.0);
    let a: f64 = after.parse().unwrap_or(0.0);
    println!("   ✓ debited: {}", b - a);
    Ok(())
}
