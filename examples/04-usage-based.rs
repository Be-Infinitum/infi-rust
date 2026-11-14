//! Usage-based billing: usage product (draft), customer, events with an idempotent eventId,
//! consumption per meter.

use beinfi::*;

#[path = "common/mod.rs"]
mod common;
use common::{show, show_json, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    step("Create and publish the usage product (draft -> publish)");
    let draft_key = common::uuid();
    let draft = client
        .products()
        .save_draft(ProductsSaveDraftInput {
            path: ProductsSaveDraftPath {
                draft_key: draft_key.clone(),
            },
            body: ProductOfferValues {
                name: "Summaries API".into(),
                pricing_model: ProductOfferValuesPricingModel::Usage,
                currency: ProductOfferValuesCurrency::BRL,
                usage: Some(ProductOfferUsage {
                    unit: "request".into(),
                    unit_amount: "0.05".into(),
                    aggregation: ProductOfferUsageAggregation::Count,
                    ..Default::default()
                }),
                ..Default::default()
            },
        })
        .await?;
    let product = client
        .products()
        .publish_draft_with(
            ProductsPublishDraftInput {
                path: ProductsPublishDraftPath { draft_key },
            },
            CallOptions::new().if_match(draft.draft_hash),
        )
        .await?;
    let product_key = product.product.key.clone().ok_or("product without key")?;
    show("productKey", &product_key);

    step("Find the meter key (config.pull)");
    let doc = client.config().pull().await?;
    let meter = doc
        .products
        .iter()
        .filter(|p| p.key == product_key)
        .find_map(|p| {
            p.meters
                .as_ref()
                .and_then(|m| m.first())
                .map(|m| m.key.clone())
        })
        .ok_or("the pull did not bring the meter of the usage product")?;
    show("meter", &meter);

    step("Create the customer with an externalId");
    let external_id = format!("user_{}", common::uniq());
    let customer = client
        .customers()
        .create(CustomersCreateInput {
            body: CustomersCreateRequest {
                product_key: Some(product_key.clone()),
                external_id: external_id.clone(),
                name: Some("Ana".into()),
                email: Some("ana@example.com".into()),
                ..Default::default()
            },
        })
        .await?;
    show_json("customer", &customer);

    step("Record usage events (and repeat one eventId)");
    let record = |event_id: &str| {
        let client = client.clone();
        let input = UsageRecordInput {
            body: FacadeUsageEvent {
                product_key: Some(product_key.clone()),
                customer_id: external_id.clone(),
                meter: meter.clone(),
                value: Some("1".into()),
                event_id: event_id.to_string(),
                ..Default::default()
            },
        };
        async move { client.usage().record(input).await }
    };
    for event_id in ["evt-1", "evt-2", "evt-3"] {
        let r = record(event_id).await?;
        show_json(event_id, &r);
    }
    let replay = record("evt-1").await?;
    show_json("evt-1 again", &replay);

    step("Read consumption per meter (expected: 3 events, not 4)");
    let usage = client
        .usage()
        .get(UsageGetInput {
            query: UsageGetQuery {
                enrollment_id: customer.enrollment_id.clone(),
                ..Default::default()
            },
        })
        .await?;
    let mut got = None;
    for m in &usage.meters {
        println!(
            "   - {}: total={} {}, events={}, value={} {}",
            m.meter,
            m.total_value,
            m.unit,
            m.event_count,
            m.total_amount.as_deref().unwrap_or("-"),
            m.currency.as_deref().unwrap_or("")
        );
        if m.meter == meter {
            got = Some(m);
        }
    }
    match got {
        Some(m) if m.event_count == 3 => {}
        other => {
            let count = other.map_or(-1, |m| m.event_count);
            return Err(format!(
                "expected 3 events, got {count} (a repeated eventId should count once)"
            )
            .into());
        }
    }
    println!("   ✓ the repeated eventId counted once");
    Ok(())
}
