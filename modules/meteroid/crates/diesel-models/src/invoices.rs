use crate::enums::{
    InvoiceExternalStatusEnum, InvoiceStatusEnum, InvoiceType, InvoicingProviderEnum,
};
use chrono::NaiveDate;
use chrono::NaiveDateTime;

use crate::customers::CustomerRow;
use diesel::{Identifiable, Insertable, Queryable, Selectable};
use uuid::Uuid;

#[derive(Debug, Identifiable, Queryable, Selectable)]
#[diesel(table_name = crate::schema::invoice)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct InvoiceRow {
    pub id: Uuid,
    pub status: InvoiceStatusEnum,
    pub external_status: Option<InvoiceExternalStatusEnum>,
    pub created_at: NaiveDateTime,
    pub updated_at: Option<NaiveDateTime>,
    pub tenant_id: Uuid,
    pub customer_id: Uuid,
    pub subscription_id: Uuid,
    pub currency: String,
    pub days_until_due: Option<i32>,
    pub external_invoice_id: Option<String>,
    pub invoice_id: Option<String>,
    pub invoicing_provider: InvoicingProviderEnum,
    pub line_items: serde_json::Value,
    pub issued: bool,
    pub issue_attempts: i32,
    pub last_issue_attempt_at: Option<NaiveDateTime>,
    pub last_issue_error: Option<String>,
    pub data_updated_at: Option<NaiveDateTime>,
    pub invoice_date: NaiveDate,
    pub amount_cents: Option<i64>,
    pub plan_version_id: Option<Uuid>,
    pub invoice_type: InvoiceType,
    pub finalized_at: Option<NaiveDateTime>,
}

#[derive(Insertable, Debug)]
#[diesel(table_name = crate::schema::invoice)]
pub struct InvoiceRowNew {
    pub id: Uuid,
    pub status: InvoiceStatusEnum,
    pub external_status: Option<InvoiceExternalStatusEnum>,
    pub tenant_id: Uuid,
    pub customer_id: Uuid,
    pub subscription_id: Uuid,
    pub currency: String,
    pub days_until_due: Option<i32>,
    pub external_invoice_id: Option<String>,
    pub invoice_id: Option<String>,
    pub invoicing_provider: InvoicingProviderEnum,
    pub line_items: serde_json::Value,
    pub issued: bool,
    pub issue_attempts: i32,
    pub last_issue_attempt_at: Option<NaiveDateTime>,
    pub last_issue_error: Option<String>,
    pub data_updated_at: Option<NaiveDateTime>,
    pub invoice_date: NaiveDate,
    pub amount_cents: Option<i64>,
    pub plan_version_id: Option<Uuid>,
    pub invoice_type: InvoiceType,
    pub finalized_at: Option<NaiveDateTime>,
}

#[derive(Debug, Queryable, Selectable)]
#[diesel(table_name = crate::schema::invoice)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct InvoiceWithPlanDetailsRow {
    pub id: Uuid,
    pub status: InvoiceStatusEnum,
    pub external_status: Option<InvoiceExternalStatusEnum>,
    pub created_at: NaiveDateTime,
    pub updated_at: Option<NaiveDateTime>,
    pub tenant_id: Uuid,
    pub customer_id: Uuid,
    pub subscription_id: Uuid,
    pub currency: String,
    pub days_until_due: Option<i32>,
    pub external_invoice_id: Option<String>,
    pub invoice_id: Option<String>,
    pub invoicing_provider: InvoicingProviderEnum,
    pub line_items: serde_json::Value,
    pub issued: bool,
    pub issue_attempts: i32,
    pub last_issue_attempt_at: Option<NaiveDateTime>,
    pub last_issue_error: Option<String>,
    pub data_updated_at: Option<NaiveDateTime>,
    pub invoice_date: NaiveDate,
    pub amount_cents: Option<i64>,
    #[diesel(select_expression = crate::schema::customer::name)]
    #[diesel(select_expression_type = crate::schema::customer::name)]
    pub customer_name: String,
    #[diesel(select_expression = crate::schema::plan::name)]
    #[diesel(select_expression_type = crate::schema::plan::name)]
    pub plan_name: String,
    #[diesel(select_expression = crate::schema::plan::external_id)]
    #[diesel(select_expression_type = crate::schema::plan::external_id)]
    pub plan_external_id: String,
    #[diesel(select_expression = crate::schema::plan_version::version)]
    #[diesel(select_expression_type = crate::schema::plan_version::version)]
    pub plan_version: i32,
}

#[derive(Debug, Queryable, Selectable)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct InvoiceWithCustomerRow {
    #[diesel(embed)]
    pub invoice: InvoiceRow,
    #[diesel(embed)]
    pub customer: CustomerRow,
}
