//! Billing as code: declare the company in a document, plan, apply and check with pull.

use beinfi::*;

#[path = "common/mod.rs"]
mod common;
use common::{show, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    let s = common::uniq();

    let document = CompanyDocument {
        schema_version: 1,
        products: vec![
            CompanyProduct {
                key: format!("subscription-{s}"),
                name: Some("Pro plan".into()),
                r#type: CompanyProductType::Item,
                pricing_model: CompanyProductPricingModel::Subscription,
                currency: Some("BRL".into()),
                base_price: Some("49.90".into()),
                billing_cycle: Some(CompanyProductBillingCycle::Monthly),
                ..Default::default()
            },
            CompanyProduct {
                key: format!("usage-{s}"),
                name: Some("Pay-per-use API".into()),
                r#type: CompanyProductType::Agent,
                pricing_model: CompanyProductPricingModel::Usage,
                currency: Some("BRL".into()),
                meters: Some(vec![CompanyMeter {
                    key: "requests".into(),
                    unit: CompanyMeterUnit::Request,
                    aggregation: CompanyMeterAggregation::Count,
                    ..Default::default()
                }]),
                prices: Some(vec![CompanyPrice {
                    meter: "requests".into(),
                    model: CompanyPriceModel::PerUnit,
                    unit_amount: Some("0.05".into()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            CompanyProduct {
                key: format!("prepaid-{s}"),
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
            },
        ],
        webhooks: None,
    };
    let body = ConfigPlanRequest {
        document: document.clone(),
        ..Default::default()
    };

    step("Plan the document (writes nothing)");
    let plan = client
        .config()
        .plan(ConfigPlanInput {
            body: body.clone(),
            ..Default::default()
        })
        .await?;
    show("planHash", &plan.plan_hash);
    show(
        "mode",
        format!(
            "{:?}, hasChanges={}, blocked={}",
            plan.mode, plan.has_changes, plan.blocked
        ),
    );
    for c in &plan.changes {
        println!(
            "   - [{:?}] {:?} {}: {}",
            c.action, c.resource, c.r#ref, c.sentence
        );
    }

    step("Apply with If-Match = planHash");
    let apply = ConfigApplyInput {
        body: ConfigApplyRequest {
            document: document.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    let applied = client
        .config()
        .apply_with(
            apply.clone(),
            CallOptions::new().if_match(plan.plan_hash.clone()),
        )
        .await?;
    show(
        "revision",
        format!(
            "#{} status={:?} mode={:?}",
            applied.revision.number, applied.revision.status, applied.revision.mode
        ),
    );

    step("Applying again with the old hash must give 412");
    let stale = common::expect_status(
        412,
        client
            .config()
            .apply_with(
                apply.clone(),
                CallOptions::new().if_match(plan.plan_hash.clone()),
            )
            .await,
    )?;
    show(
        "InfiError",
        format!(
            "status={} code={} message={:?}",
            stale.status, stale.code, stale.message
        ),
    );
    let current = stale
        .current()
        .and_then(|c| c.get("planHash"))
        .and_then(|h| h.as_str())
        .unwrap_or("(no current)");
    show("current.planHash (recomputed plan)", current);

    step("Applying without If-Match must give 428");
    let no_match = common::expect_status(428, client.config().apply(apply.clone()).await)?;
    show(
        "InfiError",
        format!("status={} code={}", no_match.status, no_match.code),
    );

    step("Pull the document from the server and check the products");
    let pulled = client.config().pull().await?;
    for p in &pulled.products {
        println!(
            "   - {} ({:?}) meters={} prices={} grants={}",
            p.key,
            p.pricing_model,
            p.meters.as_ref().map_or(0, Vec::len),
            p.prices.as_ref().map_or(0, Vec::len),
            p.grants.as_ref().map_or(0, Vec::len)
        );
    }
    for p in &document.products {
        let got = pulled
            .products
            .iter()
            .find(|g| g.key == p.key)
            .ok_or_else(|| format!("product {} did not come in the pull", p.key))?;
        if got.pricing_model != p.pricing_model {
            return Err(format!(
                "{}: pricingModel {:?} != {:?}",
                p.key, got.pricing_model, p.pricing_model
            )
            .into());
        }
    }
    println!("   ✓ the 3 products came back");

    step("Plan again: nothing to change");
    let again = client
        .config()
        .plan(ConfigPlanInput {
            body,
            ..Default::default()
        })
        .await?;
    show("hasChanges", again.has_changes);
    if again.has_changes {
        return Err("the plan after apply still has changes".into());
    }
    Ok(())
}
