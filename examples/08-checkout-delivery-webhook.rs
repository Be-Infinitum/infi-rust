//! Selling to a buyer: published product -> delivery by URL -> webhook -> checkout -> wait for payment.

use beinfi::*;

#[path = "common/mod.rs"]
mod common;
use common::{show, show_json, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    step("Publish a one-time product");
    let draft_key = common::uuid();
    let saved = client
        .products()
        .save_draft(ProductsSaveDraftInput {
            path: ProductsSaveDraftPath {
                draft_key: draft_key.clone(),
            },
            body: ProductOfferValues {
                name: "PDF guide".into(),
                pricing_model: ProductOfferValuesPricingModel::OneTime,
                currency: ProductOfferValuesCurrency::BRL,
                base_price: Some("19.00".into()),
                ..Default::default()
            },
        })
        .await?;
    let published = client
        .products()
        .publish_draft_with(
            ProductsPublishDraftInput {
                path: ProductsPublishDraftPath { draft_key },
            },
            CallOptions::new().if_match(saved.draft_hash),
        )
        .await?;
    let product_id = published.product.id.clone();
    if product_id.is_empty() {
        return Err("the product was not published with an id".into());
    }
    show("productId", &product_id);

    step("Set the delivery: the buyer gets this URL after paying");
    client
        .products()
        .set_deliverable(ProductsSetDeliverableInput {
            path: ProductsSetDeliverablePath {
                product_id: product_id.clone(),
            },
            body: PutDeliverableRequest {
                kind: PutDeliverableRequestKind::Link,
                url: Some("https://example.com/guide.pdf".into()),
                ..Default::default()
            },
        })
        .await?;
    let deliverable = client
        .products()
        .get_deliverable(ProductsGetDeliverableInput {
            path: ProductsGetDeliverablePath {
                product_id: product_id.clone(),
            },
        })
        .await?;
    show_json("delivery", &deliverable);

    step("Subscribe a webhook to hear about the payment");
    // In the sandbox it applies at once and returns the secret one time; in live it
    // returns approvalUrl for a person to approve. If the sandbox does not have the
    // secret vault configured yet, it answers 503: the step is skipped.
    let result = async {
        let hook = client
            .webhooks()
            .subscribe(WebhooksSubscribeInput {
                body: WebhookSubscribeRequest {
                    url: "https://example.com/hooks/infi".into(),
                    events: vec!["payment.confirmed".into()],
                },
            })
            .await?;
        let mut desc = format!("{:?}", hook.status);
        if hook.secret.as_deref().is_some_and(|s| !s.is_empty()) {
            desc.push_str(" (secret received)");
        }
        if let Some(url) = hook.approval_url.as_deref().filter(|u| !u.is_empty()) {
            desc.push_str(&format!(" approve at {url}"));
        }
        show("webhook", desc);
        client.webhooks().list().await
    }
    .await;
    match result {
        Err(e) if e.status() == Some(503) => {
            show(
                "webhook",
                "503: the sandbox does not store webhook secrets yet",
            );
        }
        Err(e) => return Err(e.into()),
        Ok(list) => {
            let urls: Vec<&str> = list.data.iter().map(|w| w.url.as_str()).collect();
            show("webhooks", format!("{urls:?}"));
        }
    }

    step("Open a buyer's checkout");
    let checkout = client
        .checkouts()
        .create(CheckoutsCreateInput {
            body: CheckoutCreateRequest {
                product_id: product_id.clone(),
                customer: CheckoutCreateRequestCustomer {
                    external_id: format!("buyer_{}", common::uniq()),
                    email: Some("buyer@example.com".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        })
        .await?;
    show(
        "checkout",
        format!(
            "{} {} {} {}",
            checkout.id, checkout.status, checkout.total, checkout.currency
        ),
    );
    show("send the buyer to", &checkout.url);
    if !checkout.url.contains(&format!("/invoices/{}", checkout.id)) {
        return Err("the checkout url does not point to the invoice".into());
    }

    step("Read the checkout: this is how code waits for the payment");
    let again = client
        .checkouts()
        .get(CheckoutsGetInput {
            path: CheckoutsGetPath {
                checkout_id: checkout.id.clone(),
            },
        })
        .await?;
    show("status", &again.status);
    show("payments", again.payments.len());
    if again.id != checkout.id {
        return Err("the checkout that was read is not the one that was created".into());
    }
    Ok(())
}
