//! One-time product: product draft -> preview -> link draft -> publish with If-Match.

use beinfi::*;

#[path = "common/mod.rs"]
mod common;
use common::{show, show_json, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    step("Save the one-time product draft");
    let product_key = common::uuid();
    let saved = client
        .products()
        .save_draft(ProductsSaveDraftInput {
            path: ProductsSaveDraftPath {
                draft_key: product_key.clone(),
            },
            body: ProductOfferValues {
                name: "1h mentoring".into(),
                description: Some("One individual session".into()),
                pricing_model: ProductOfferValuesPricingModel::OneTime,
                currency: ProductOfferValuesCurrency::BRL,
                base_price: Some("150.00".into()),
                ..Default::default()
            },
        })
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
    show(
        "offer",
        format!(
            "{} — {} {:?}",
            preview.offer.name,
            preview.offer.base_price.as_deref().unwrap_or(""),
            preview.offer.currency
        ),
    );
    show("published", preview.published);

    step("Save the link draft pointing at the product draft");
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
                success_url: Some("https://example.com/thanks".into()),
                ..Default::default()
            },
        })
        .await?;
    show_json("link preview", &link.preview);
    show("link draftHash", &link.draft_hash);

    step("Publishing the link without If-Match must give 428");
    let publish = LinksPublishDraftInput {
        path: LinksPublishDraftPath {
            draft_key: link_key.clone(),
        },
    };
    let e428 = common::expect_status(428, client.links().publish_draft(publish.clone()).await)?;
    show(
        "InfiError",
        format!("status={} code={}", e428.status, e428.code),
    );

    step("Publish the link with If-Match = draftHash");
    let published = client
        .links()
        .publish_draft_with(
            publish,
            CallOptions::new().if_match(link.draft_hash.clone()),
        )
        .await?;
    show("published", published.published);
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
