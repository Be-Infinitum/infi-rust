//! Customer session (portal): create, verify, revoke, verify again (fails).

use beinfi::*;

#[path = "common/mod.rs"]
mod common;
use common::{show, show_json, step, BoxError};

#[tokio::main]
async fn main() {
    common::run(run).await;
}

async fn run(client: Client) -> Result<(), BoxError> {
    step("Create a product and a customer");
    let draft_key = common::uuid();
    let draft = client
        .products()
        .save_draft(ProductsSaveDraftInput {
            path: ProductsSaveDraftPath {
                draft_key: draft_key.clone(),
            },
            body: ProductOfferValues {
                name: "Portal plan".into(),
                pricing_model: ProductOfferValuesPricingModel::Subscription,
                currency: ProductOfferValuesCurrency::BRL,
                base_price: Some("19.90".into()),
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
    let external_id = format!("user_{}", common::uniq());
    let customer = client
        .customers()
        .create(CustomersCreateInput {
            body: CustomersCreateRequest {
                product_id: Some(product.product.id.clone()),
                external_id: external_id.clone(),
                name: Some("Bia".into()),
                email: Some("bia@example.com".into()),
                ..Default::default()
            },
        })
        .await?;
    show_json("customer", &customer);

    step("Create the session for the externalId");
    let session = client
        .customer_sessions()
        .create(CustomerSessionsCreateInput {
            body: CustomerSessionsCreateRequest {
                external_id: external_id.clone(),
                ..Default::default()
            },
        })
        .await?;
    let token: String = session.token.chars().take(12).collect();
    show("token", format!("{token}…"));
    show(
        "scope / origin / expires",
        format!(
            "{:?} / {:?} / {}",
            session.scope, session.origin, session.expires_at
        ),
    );

    let verify = || {
        let client = client.clone();
        let token = session.token.clone();
        async move {
            client
                .customer_sessions()
                .verify(CustomerSessionsVerifyInput {
                    headers: CustomerSessionsVerifyHeaders {
                        x_buyer_token: token,
                    },
                })
                .await
        }
    };

    step("Verify the token");
    let verified = verify().await?;
    show_json("buyer", &verified.buyer);
    show("access", verified.access.len());
    if !verified.buyer.external_ids.contains(&external_id) {
        return Err("the token does not point to the externalId that was created".into());
    }

    step("Revoke the token");
    let revoked = client
        .customer_sessions()
        .revoke(CustomerSessionsRevokeInput {
            body: CustomerSessionsRevokeRequest {
                token: Some(session.token.clone()),
                ..Default::default()
            },
        })
        .await?;
    show("revoked", revoked.revoked);

    step("Verify again (must fail)");
    let e = common::expect_status(401, verify().await)?;
    show("InfiError", format!("status={} code={}", e.status, e.code));
    Ok(())
}
