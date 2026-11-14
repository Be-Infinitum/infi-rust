//! Reconciliation: walk whole listings with paginate (customers and payments).

use beinfi::*;
use std::collections::HashSet;

#[path = "common/mod.rs"]
mod common;
use common::{show, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    step("Create a product and ~25 customers");
    let draft_key = common::uuid();
    let draft = client
        .products()
        .save_draft(ProductsSaveDraftInput {
            path: ProductsSaveDraftPath {
                draft_key: draft_key.clone(),
            },
            body: ProductOfferValues {
                name: "Reconciliation product".into(),
                pricing_model: ProductOfferValuesPricingModel::Subscription,
                currency: ProductOfferValuesCurrency::BRL,
                base_price: Some("10.00".into()),
                billing_cycle: Some(ProductOfferValuesBillingCycle::Monthly),
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
    let s = common::uniq();
    const TOTAL: usize = 25;
    for i in 1..=TOTAL {
        client
            .customers()
            .create(CustomersCreateInput {
                body: CustomersCreateRequest {
                    product_id: Some(product.product.id.clone()),
                    external_id: format!("cli_{s}_{i:02}"),
                    ..Default::default()
                },
            })
            .await?;
    }
    show("created", TOTAL);

    step("Paginate customers.list with limit 10");
    let client_ref = &client;
    let list_customers = move |cursor: Option<String>| async move {
        let l = client_ref
            .customers()
            .list(CustomersListInput {
                query: CustomersListQuery {
                    limit: Some(10),
                    cursor,
                    ..Default::default()
                },
            })
            .await?;
        Ok(Page::new(l.data, l.next_cursor))
    };
    // The sandbox account may already hold customers from other runs: the proof is that
    // pagination repeats nobody and that the 25 created now all show up.
    let mine = format!("cli_{s}_");
    let mut seen = 0;
    let mut ids = HashSet::new();
    let mut ours = HashSet::new();
    let mut customers = paginate(list_customers);
    while let Some(c) = customers.next().await {
        let c = c?;
        seen += 1;
        if c.external_id.starts_with(&mine) {
            ours.insert(c.id.clone());
        }
        ids.insert(c.id);
    }
    let first = client
        .customers()
        .list(CustomersListInput {
            query: CustomersListQuery {
                limit: Some(10),
                ..Default::default()
            },
        })
        .await?;
    show("items seen", seen);
    show("distinct ids", ids.len());
    show("created now", ours.len());
    show(
        "first page",
        format!(
            "{} items, nextCursor={}, summary={}",
            first.data.len(),
            if first.next_cursor.is_some() {
                "yes"
            } else {
                "no"
            },
            serde_json::to_string(&first.summary).unwrap_or_default()
        ),
    );
    if ids.len() != seen {
        return Err(format!(
            "pagination repeated customers: {seen} items, {} distinct",
            ids.len()
        )
        .into());
    }
    if ours.len() != TOTAL {
        return Err(format!(
            "expected the {TOTAL} customers created now, saw {}",
            ours.len()
        )
        .into());
    }
    println!(
        "   ✓ {seen} customers in {} pages, no repeats, including the {TOTAL} created now",
        seen.div_ceil(10)
    );

    step("Paginate payments.list");
    let mut payments = 0;
    let mut pages = paginate(move |cursor: Option<String>| async move {
        let l = client_ref
            .payments()
            .list(PaymentsListInput {
                query: PaymentsListQuery {
                    limit: Some(10),
                    cursor,
                    ..Default::default()
                },
            })
            .await?;
        Ok(Page::new(l.data, l.next_cursor))
    });
    while let Some(p) = pages.next().await {
        p?;
        payments += 1;
    }
    show("payments", payments);
    if payments == 0 {
        println!("   (empty: in the local sandbox no payer paid a link; pagination runs the same, there are just no items)");
    }
    Ok(())
}
