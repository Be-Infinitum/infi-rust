//! Monthly subscription: the same draft -> preview -> publish flow.

use beinfi::*;

#[path = "common/mod.rs"]
mod common;
use common::{show, show_json, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

fn offer(description: &str) -> ProductOfferValues {
    ProductOfferValues {
        name: "Monthly club".into(),
        description: Some(description.into()),
        pricing_model: ProductOfferValuesPricingModel::Subscription,
        currency: ProductOfferValuesCurrency::BRL,
        base_price: Some("29.90".into()),
        billing_cycle: Some(ProductOfferValuesBillingCycle::Monthly),
        ..Default::default()
    }
}

async fn run(client: Client) -> Result<(), BoxError> {
    step("Save the monthly subscription draft");
    let product_key = common::uuid();
    let save = |description: &str| ProductsSaveDraftInput {
        path: ProductsSaveDraftPath {
            draft_key: product_key.clone(),
        },
        body: offer(description),
    };
    let saved = client
        .products()
        .save_draft(save("New content every month"))
        .await?;
    show("draftHash", &saved.draft_hash);

    step("Read the product preview with ?hash=");
    let preview = client
        .products()
        .get_draft(ProductsGetDraftInput {
            path: ProductsGetDraftPath {
                draft_key: product_key.clone(),
            },
            query: ProductsGetDraftQuery {
                hash: Some(saved.draft_hash.clone()),
            },
        })
        .await?;
    let cycle = preview
        .offer
        .billing_cycle
        .as_ref()
        .map(|c| format!("{c:?}"))
        .unwrap_or_default();
    show(
        "offer",
        format!(
            "{} — {} {:?}/{}",
            preview.offer.name,
            preview.offer.base_price.as_deref().unwrap_or(""),
            preview.offer.currency,
            cycle
        ),
    );

    step("Save the link draft");
    let link_key = common::uuid();
    let link = client
        .links()
        .save_draft(LinksSaveDraftInput {
            path: LinksSaveDraftPath {
                draft_key: link_key.clone(),
            },
            body: SaveLinkDraftRequest {
                product: SaveLinkDraftRequestProduct {
                    draft_key: Some(product_key.clone()),
                    ..Default::default()
                },
                success_url: Some("https://example.com/welcome".into()),
                cancel_url: Some("https://example.com/back".into()),
                ..Default::default()
            },
        })
        .await?;
    show_json("link preview", &link.preview);

    step("Editing the product after the preview invalidates the link hash (412)");
    let edited = client
        .products()
        .save_draft_with(
            save("New content every month + community"),
            CallOptions::new().if_match(saved.draft_hash.clone()),
        )
        .await?;
    show("new product draftHash", &edited.draft_hash);
    let publish = LinksPublishDraftInput {
        path: LinksPublishDraftPath {
            draft_key: link_key.clone(),
        },
    };
    let stale = common::expect_status(
        412,
        client
            .links()
            .publish_draft_with(
                publish.clone(),
                CallOptions::new().if_match(link.draft_hash.clone()),
            )
            .await,
    )?;
    show(
        "InfiError",
        format!("status={} code={}", stale.status, stale.code),
    );

    step("Read the link preview again (new hash) and publish");
    let fresh = client
        .links()
        .get_draft(LinksGetDraftInput {
            path: LinksGetDraftPath {
                draft_key: link_key.clone(),
            },
            ..Default::default()
        })
        .await?;
    let published = client
        .links()
        .publish_draft_with(
            publish,
            CallOptions::new().if_match(fresh.draft_hash.clone()),
        )
        .await?;
    let url = published
        .link
        .as_ref()
        .and_then(|l| l.url.clone())
        .unwrap_or_default();
    show("link URL", if url.is_empty() { "(no url)" } else { &url });
    if !published.published || url.is_empty() {
        return Err("the link was not published with a url".into());
    }
    Ok(())
}
