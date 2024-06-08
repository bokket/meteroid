use crate::domain::enums::{InvoiceExternalStatusEnum, InvoiceType};
use crate::errors::StoreError;
use crate::store::Store;
use crate::{domain, StoreResult};
use chrono::NaiveDateTime;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_models::enums::{MrrMovementType, SubscriptionEventType};
use diesel_models::PgConn;
use error_stack::Report;

use crate::domain::{
    CursorPaginatedVec, CursorPaginationRequest, Invoice, InvoiceNew, InvoiceWithCustomer,
    InvoiceWithPlanDetails, OrderByRequest, PaginatedVec, PaginationRequest,
};
use common_eventbus::Event;
use diesel_models::invoices::{InvoiceRow, InvoiceRowNew};
use diesel_models::subscriptions::SubscriptionRow;
use tracing_log::log;
use uuid::Uuid;

#[async_trait::async_trait]
pub trait InvoiceInterface {
    async fn find_invoice_by_id(
        &self,
        tenant_id: Uuid,
        invoice_id: Uuid,
    ) -> StoreResult<InvoiceWithPlanDetails>;

    async fn list_invoices(
        &self,
        tenant_id: Uuid,
        customer_id: Option<Uuid>,
        status: Option<domain::enums::InvoiceStatusEnum>,
        query: Option<String>,
        order_by: OrderByRequest,
        pagination: PaginationRequest,
    ) -> StoreResult<PaginatedVec<InvoiceWithCustomer>>;

    async fn insert_invoice(&self, invoice: InvoiceNew) -> StoreResult<Invoice>;

    async fn insert_invoice_batch(&self, invoice: Vec<InvoiceNew>) -> StoreResult<Vec<Invoice>>;

    async fn update_invoice_external_status(
        &self,
        invoice_id: Uuid,
        tenant_id: Uuid,
        external_status: InvoiceExternalStatusEnum,
    ) -> StoreResult<()>;

    async fn list_invoices_to_finalize(
        &self,
        pagination: CursorPaginationRequest,
    ) -> StoreResult<CursorPaginatedVec<Invoice>>;

    async fn finalize_invoice(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        lines: serde_json::Value,
    ) -> StoreResult<()>;

    async fn list_outdated_invoices(
        &self,
        pagination: CursorPaginationRequest,
    ) -> StoreResult<CursorPaginatedVec<Invoice>>;

    async fn update_invoice_lines(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        lines: serde_json::Value,
    ) -> StoreResult<()>;

    async fn list_invoices_to_issue(
        &self,
        max_attempts: i32,
        pagination: CursorPaginationRequest,
    ) -> StoreResult<CursorPaginatedVec<Invoice>>;

    async fn invoice_issue_success(&self, id: Uuid, tenant_id: Uuid) -> StoreResult<()>;

    async fn invoice_issue_error(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        last_issue_error: &str,
    ) -> StoreResult<()>;

    async fn update_pending_finalization_invoices(&self, now: NaiveDateTime) -> StoreResult<()>;
}

#[async_trait::async_trait]
impl InvoiceInterface for Store {
    async fn find_invoice_by_id(
        &self,
        tenant_id: Uuid,
        invoice_id: Uuid,
    ) -> StoreResult<InvoiceWithPlanDetails> {
        let mut conn = self.get_conn().await?;

        InvoiceRow::find_by_id(&mut conn, tenant_id, invoice_id)
            .await
            .map_err(Into::into)
            .map(Into::into)
    }

    async fn list_invoices(
        &self,
        tenant_id: Uuid,
        customer_id: Option<Uuid>,
        status: Option<domain::enums::InvoiceStatusEnum>,
        query: Option<String>,
        order_by: OrderByRequest,
        pagination: PaginationRequest,
    ) -> StoreResult<PaginatedVec<InvoiceWithCustomer>> {
        let mut conn = self.get_conn().await?;

        let rows = InvoiceRow::list(
            &mut conn,
            tenant_id,
            customer_id,
            status.map(Into::into),
            query,
            order_by.into(),
            pagination.into(),
        )
        .await
        .map_err(Into::<Report<StoreError>>::into)?;

        let res: PaginatedVec<InvoiceWithCustomer> = PaginatedVec {
            items: rows
                .items
                .into_iter()
                .map(|s| s.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            total_pages: rows.total_pages,
            total_results: rows.total_results,
        };

        Ok(res)
    }

    async fn insert_invoice(&self, invoice: InvoiceNew) -> StoreResult<Invoice> {
        let mut conn = self.get_conn().await?;

        let insertable_invoice: InvoiceRowNew = invoice.into();

        let inserted: Invoice = insertable_invoice
            .insert(&mut conn)
            .await
            .map_err(Into::<Report<StoreError>>::into)
            .map(Into::into)?;

        process_mrr(&inserted, &mut conn).await?;

        Ok(inserted)
    }

    async fn insert_invoice_batch(&self, invoice: Vec<InvoiceNew>) -> StoreResult<Vec<Invoice>> {
        let mut conn = self.get_conn().await?;

        let insertable_invoice: Vec<InvoiceRowNew> =
            invoice.into_iter().map(|c| c.into()).collect();

        let inserted: Vec<Invoice> =
            InvoiceRow::insert_invoice_batch(&mut conn, insertable_invoice)
                .await
                .map_err(Into::<Report<StoreError>>::into)
                .map(|v| v.into_iter().map(Into::into).collect())?;

        for inv in &inserted {
            process_mrr(inv, &mut conn).await?; // TODO batch
        }

        // TODO update subscription mrr

        Ok(inserted)
    }

    async fn update_invoice_external_status(
        &self,
        invoice_id: Uuid,
        tenant_id: Uuid,
        external_status: InvoiceExternalStatusEnum,
    ) -> StoreResult<()> {
        self.transaction(|conn| {
            async move {
                InvoiceRow::update_external_status(
                    conn,
                    invoice_id,
                    tenant_id,
                    external_status.clone().into(),
                )
                .await
                .map_err(Into::<Report<StoreError>>::into)?;

                if external_status == InvoiceExternalStatusEnum::Paid {
                    let invoice = InvoiceRow::find_by_id(conn, tenant_id, invoice_id)
                        .await
                        .map_err(Into::<Report<StoreError>>::into)?;

                    SubscriptionRow::activate_subscription(
                        conn,
                        invoice.subscription_id,
                        tenant_id,
                    )
                    .await
                    .map_err(Into::<Report<StoreError>>::into)?;
                }

                Ok(())
            }
            .scope_boxed()
        })
        .await
    }

    async fn list_invoices_to_finalize(
        &self,
        pagination: CursorPaginationRequest,
    ) -> StoreResult<CursorPaginatedVec<Invoice>> {
        let mut conn = self.get_conn().await?;

        let invoices = InvoiceRow::list_to_finalize(&mut conn, pagination.into())
            .await
            .map_err(Into::<Report<StoreError>>::into)?;

        let res: CursorPaginatedVec<Invoice> = CursorPaginatedVec {
            items: invoices.items.into_iter().map(|s| s.into()).collect(),
            next_cursor: invoices.next_cursor,
        };

        Ok(res)
    }

    async fn finalize_invoice(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        lines: serde_json::Value,
    ) -> StoreResult<()> {
        let mut conn = self.get_conn().await?;

        let _ = InvoiceRow::finalize(&mut conn, id, tenant_id, lines)
            .await
            .map_err(Into::<Report<StoreError>>::into)?;

        let _ = self
            .eventbus
            .publish(Event::invoice_finalized(id, tenant_id))
            .await;

        Ok(())
    }

    async fn list_outdated_invoices(
        &self,
        pagination: CursorPaginationRequest,
    ) -> StoreResult<CursorPaginatedVec<Invoice>> {
        let mut conn = self.get_conn().await?;

        let invoices = InvoiceRow::list_outdated(&mut conn, pagination.into())
            .await
            .map_err(Into::<Report<StoreError>>::into)?;

        let res: CursorPaginatedVec<Invoice> = CursorPaginatedVec {
            items: invoices.items.into_iter().map(|s| s.into()).collect(),
            next_cursor: invoices.next_cursor,
        };

        Ok(res)
    }

    async fn update_invoice_lines(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        lines: serde_json::Value,
    ) -> StoreResult<()> {
        let mut conn = self.get_conn().await?;

        InvoiceRow::update_lines(&mut conn, id, tenant_id, lines)
            .await
            .map(|_| ())
            .map_err(Into::<Report<StoreError>>::into)
    }

    async fn list_invoices_to_issue(
        &self,
        max_attempts: i32,
        pagination: CursorPaginationRequest,
    ) -> StoreResult<CursorPaginatedVec<Invoice>> {
        let mut conn = self.get_conn().await?;

        let invoices = InvoiceRow::list_to_issue(&mut conn, max_attempts, pagination.into())
            .await
            .map_err(Into::<Report<StoreError>>::into)?;

        let res: CursorPaginatedVec<Invoice> = CursorPaginatedVec {
            items: invoices.items.into_iter().map(|s| s.into()).collect(),
            next_cursor: invoices.next_cursor,
        };

        Ok(res)
    }

    async fn invoice_issue_success(&self, id: Uuid, tenant_id: Uuid) -> StoreResult<()> {
        let mut conn = self.get_conn().await?;

        InvoiceRow::issue_success(&mut conn, id, tenant_id)
            .await
            .map(|_| ())
            .map_err(Into::<Report<StoreError>>::into)
    }

    async fn invoice_issue_error(
        &self,
        id: Uuid,
        tenant_id: Uuid,
        last_issue_error: &str,
    ) -> StoreResult<()> {
        let mut conn = self.get_conn().await?;

        InvoiceRow::issue_error(&mut conn, id, tenant_id, last_issue_error)
            .await
            .map(|_| ())
            .map_err(Into::<Report<StoreError>>::into)
    }

    async fn update_pending_finalization_invoices(&self, now: NaiveDateTime) -> StoreResult<()> {
        let mut conn = self.get_conn().await?;

        InvoiceRow::update_pending_finalization(&mut conn, now)
            .await
            .map(|_| ())
            .map_err(Into::<Report<StoreError>>::into)
    }
}

/*

TODO special cases :
- cancellation/all invoice => all mrr logs after that should be cancelled, unless reactivation
- cancellation : recalculate the mrr delta (as the one in the event was calculated before the invoice was created)
- consolidation if multiple events in the same day. Ex: new business + expansion = new business, or cancellation + reactivation => nothing
 */
async fn process_mrr(inserted: &domain::Invoice, conn: &mut PgConn) -> StoreResult<()> {
    log::info!("Processing MRR logs for invoice {}", inserted.id);
    if inserted.invoice_type == InvoiceType::Recurring
        || inserted.invoice_type == InvoiceType::Adjustment
    {
        let subscription_events = diesel_models::subscription_events::SubscriptionEventRow::fetch_by_subscription_id_and_date(
            conn,
            inserted.subscription_id,
            inserted.invoice_date,
        )
            .await
            .map_err(Into::<Report<StoreError>>::into)?;

        log::info!("subscription_events len {}", subscription_events.len());

        let mut mrr_logs = vec![];

        for event in subscription_events {
            let mrr_delta = match event.mrr_delta {
                None | Some(0) => continue,
                Some(c) => c,
            };

            let movement_type = match event.event_type {
                // TODO
                // SubscriptionEventType::Created => continue,
                SubscriptionEventType::Created => MrrMovementType::NewBusiness, // TODO
                SubscriptionEventType::Activated => MrrMovementType::NewBusiness,
                SubscriptionEventType::Switch => {
                    if mrr_delta > 0 {
                        MrrMovementType::Expansion
                    } else {
                        MrrMovementType::Contraction
                    }
                }
                SubscriptionEventType::Cancelled => MrrMovementType::Churn,
                SubscriptionEventType::Reactivated => MrrMovementType::Reactivation,
                SubscriptionEventType::Updated => {
                    if mrr_delta > 0 {
                        MrrMovementType::Expansion
                    } else {
                        MrrMovementType::Contraction
                    }
                }
            };

            // TODO proper description from event_type + details
            let description = match event.event_type {
                SubscriptionEventType::Created => "Subscription created",
                SubscriptionEventType::Activated => "Subscription activated",
                SubscriptionEventType::Switch => "Switched plan",
                SubscriptionEventType::Cancelled => "Subscription cancelled",
                SubscriptionEventType::Reactivated => "Subscription reactivated",
                SubscriptionEventType::Updated => "Subscription updated",
            };

            let new_log = diesel_models::bi::BiMrrMovementLogRowNew {
                id: Uuid::now_v7(),
                description: description.to_string(),
                movement_type,
                net_mrr_change: mrr_delta,
                currency: inserted.currency.clone(),
                applies_to: inserted.invoice_date,
                invoice_id: inserted.id,
                credit_note_id: None,
                plan_version_id: inserted.plan_version_id.unwrap(), // TODO
                tenant_id: inserted.tenant_id,
            };

            mrr_logs.push(new_log);
        }

        let mrr_delta_cents: i64 = mrr_logs.iter().map(|l| l.net_mrr_change).sum();

        diesel_models::bi::BiMrrMovementLogRow::insert_movement_log_batch(conn, mrr_logs)
            .await
            .map_err(Into::<Report<StoreError>>::into)?;

        diesel_models::subscriptions::SubscriptionRow::update_subscription_mrr_delta(
            conn,
            inserted.subscription_id,
            mrr_delta_cents,
        )
        .await
        .map_err(Into::<Report<StoreError>>::into)?;
    }
    Ok(())
}
