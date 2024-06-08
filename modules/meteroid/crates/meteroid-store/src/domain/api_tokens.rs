use chrono::NaiveDateTime;
use o2o::o2o;
use uuid::Uuid;

use diesel_models::api_tokens::ApiTokenRow;
use diesel_models::api_tokens::ApiTokenRowNew;

#[derive(Debug, o2o)]
#[from_owned(ApiTokenRowNew)]
pub struct ApiTokenNew {
    pub name: String,
    pub created_by: Uuid,
    pub tenant_id: Uuid,
}

#[derive(Debug, o2o)]
#[from_owned(ApiTokenRow)]
#[owned_into(ApiTokenRow)]
pub struct ApiToken {
    pub id: Uuid,
    pub name: String,
    pub created_at: NaiveDateTime,
    pub created_by: Uuid,
    pub tenant_id: Uuid,
    pub hash: String,
    pub hint: String,
}
