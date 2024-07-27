use std::collections::HashMap;
use std::str::FromStr;

use chrono::{Datelike, Days, Months};
use opentelemetry::propagation::Injector;
use rust_decimal::Decimal;
use testcontainers::clients::Cli;
use tonic::Request;
use uuid::{uuid, Uuid};

use metering::utils::datetime_to_timestamp;
use metering_grpc::meteroid::metering::v1::{event::CustomerId, Event, IngestRequest};
use meteroid::mapping::common::chrono_to_date;
use meteroid::models::{InvoiceLine, InvoiceLinePeriod};
use meteroid_grpc::meteroid::api;
use meteroid_grpc::meteroid::api::billablemetrics::v1::aggregation::AggregationType;
use meteroid_grpc::meteroid::api::billablemetrics::v1::segmentation_matrix::{
    Dimension, Matrix, SegmentationMatrixDouble,
};
use meteroid_grpc::meteroid::api::billablemetrics::v1::{
    Aggregation, CreateBillableMetricRequest, SegmentationMatrix,
};
use meteroid_grpc::meteroid::api::plans::v1::PlanType;
use meteroid_store::domain::enums::{InvoiceStatusEnum, InvoiceType, InvoicingProviderEnum};
use meteroid_store::domain::{Invoice, InvoiceNew, OrderByRequest, PaginationRequest};
use meteroid_store::repositories::InvoiceInterface;
use meteroid_store::Store;

use crate::metering_it;
use crate::{helpers, meteroid_it};

/*
Plan with Capacity
(aka fixed advance fee + usage fee)

In this case, we are at the end of p1, just before workers run.
So we have :
- a finalized invoice for p1
- a draft invoice for p2

After the workers run we will have :
- a finalized invoice for p1
- a finalized invoice for p2, with the advance for p2, and the arrear for p1
- a draft invoice for p3

 */

#[tokio::test]
#[ignore] // create subscription fails
async fn test_metering_e2e() {
    helpers::init::logging();

    let docker = Cli::default();

    // we start pg, clickhouse, kafka

    let (_pg_container, postgres_connection_string) =
        meteroid_it::container::start_postgres(&docker);

    let (_kafka_container, kafka_port) = metering_it::container::start_kafka(&docker)
        .await
        .expect("Could not start kafka");

    let (_clickhouse_container, clickhouse_port) =
        metering_it::container::start_clickhouse(&docker).await;

    metering_it::kafka::create_topic(kafka_port, "meteroid-events-raw")
        .await
        .expect("Could not create topic");

    // we start meteroid and metering

    let meteroid_port = helpers::network::free_local_port().expect("Could not get free port");
    let metering_port = helpers::network::free_local_port().expect("Could not get free port");

    let meteroid_setup = meteroid_it::container::start_meteroid_with_port(
        meteroid_port,
        metering_port,
        postgres_connection_string,
        meteroid_it::container::SeedLevel::PRODUCT,
    )
    .await;

    let store = meteroid_setup.store;

    let jwt_auth = meteroid_it::svc_auth::login(meteroid_setup.channel.clone()).await;

    let mut meteroid_clients = meteroid_it::clients::AllClients::from_channel(
        meteroid_setup.channel.clone(),
        jwt_auth.token.clone().as_str(),
        "a712afi5lzhk",
    );

    let metering_config = metering_it::config::mocked_config(
        meteroid_port,
        metering_port,
        clickhouse_port,
        kafka_port,
        "meteroid-events-raw".to_string(),
    );

    let metering_setup = metering_it::container::start_metering(metering_config.clone()).await;

    let api_key = "pv_sand_9XzHg0EYO2Usy9ITU6bbhBnkYYbx/2vO7XtUUeQ7Wq9EZCAbBG";

    let mut metering_clients = metering_it::clients::AllClients::from_channel(
        metering_setup.channel.clone(),
        api_key,
        &metering_config.internal_auth,
    );

    let tenant_uuid = uuid!("018c2c82-3df1-7e84-9e05-6e141d0e751a");
    let tenant_id = tenant_uuid.to_string();

    let customer_1 = "018c345f-7324-7cd2-a692-78e5ab9158e0".to_string();
    let customer_2 = "018c345f-dff1-7857-b988-6c792ed6fa3f".to_string();

    let now = chrono::Utc::now();

    // period 2 started yesterday.
    // so after workers we expect a draft for period 2, and a finalized/issued for period 1
    // we will create the draft invoice for period 1 manually, as the draft worker only check last 7 days
    let period_2_start = now - chrono::Duration::days(1);
    let period_2_end = period_2_start.checked_add_months(Months::new(1)).unwrap();

    let billing_day = period_2_start.day();

    let period_1_start = period_2_start.checked_sub_days(Days::new(20)).unwrap();
    let period_1_end = period_2_start;

    // we consider a billing period 1, customer 1, inference endpoint
    let events = vec![
        (
            &customer_1,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "20"),
            ],
            period_1_start,
        ),
        (
            &customer_1,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "150"),
            ],
            period_1_start.checked_add_days(Days::new(1)).unwrap(),
        ),
        (
            &customer_1,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "70"),
            ],
            period_1_start.checked_add_days(Days::new(10)).unwrap(),
        ),
        (
            &customer_1,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "9"),
            ],
            period_2_start.checked_sub_days(Days::new(1)).unwrap(),
        ),
        // out of period
        (
            &customer_1,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "25000"),
            ],
            period_2_start,
        ),
        // other customer
        (
            &customer_2,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "25000"),
            ],
            period_1_start.checked_add_days(Days::new(10)).unwrap(),
        ),
        // other event type
        (
            &customer_1,
            "api_response",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_3.5"),
                ("tokens", "25000"),
            ],
            period_1_start.checked_add_days(Days::new(10)).unwrap(),
        ),
        // other endpoint
        (
            &customer_1,
            "api_response",
            vec![
                ("endpoint", "embedding"),
                ("model", "gpt_3.5"),
                ("tokens", "25000"),
            ],
            period_1_start.checked_add_days(Days::new(10)).unwrap(),
        ),
        // other model
        (
            &customer_2,
            "api_calls",
            vec![
                ("endpoint", "inference"),
                ("model", "gpt_4"),
                ("tokens", "25000"),
            ],
            period_1_start.checked_add_days(Days::new(10)).unwrap(),
        ),
    ];

    let events_mapped: Vec<Event> = events
        .into_iter()
        .map(|(customer, event_name, properties, timestamp)| Event {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_name: event_name.to_string(),
            customer_id: Some(CustomerId::MeteroidCustomerId(customer.clone())),
            timestamp: Some(datetime_to_timestamp(timestamp)),
            properties: {
                let mut map = HashMap::new();
                for (key, value) in properties {
                    map.set(key, value.to_string());
                }
                map
            },
        })
        .collect();

    // we ingest events in metering
    let ingested = metering_clients
        .events
        .ingest(Request::new(IngestRequest {
            events: events_mapped,
            allow_backfilling: true,
        }))
        .await
        .expect("Could not ingest events");

    let ingested = ingested.into_inner();

    assert_eq!(ingested.failures.len(), 0);

    // TODO loop & count(*) until it is ingested
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // we create a meter
    let created_metric = meteroid_clients
        .metrics
        .create_billable_metric(Request::new(CreateBillableMetricRequest {
            name: "inference api calls".to_string(),
            code: "api_calls".to_string(),
            description: None,
            aggregation: Some(Aggregation {
                aggregation_type: AggregationType::Sum as i32,
                aggregation_key: Some("tokens".to_string()),
                unit_conversion: None,
            }),
            segmentation_matrix: Some(SegmentationMatrix {
                // TODO simplify. Also, Vec<Dimension / LinkedDimension> ?
                matrix: Some(Matrix::Double(SegmentationMatrixDouble {
                    dimension1: Some(Dimension {
                        key: "endpoint".to_string(),
                        values: vec!["inference".to_string()],
                    }),
                    dimension2: Some(Dimension {
                        key: "model".to_string(),
                        values: vec!["gpt_3.5".to_string()],
                    }),
                })),
            }),
            usage_group_key: None,
            family_external_id: "default".to_string(),
        }))
        .await
        .expect("Could not create meter");

    let created_metric = created_metric.into_inner();

    let metric_id = created_metric.billable_metric.as_ref().unwrap().id.clone();

    // we validate that it was created in clickhouse

    let mut clickhouse_client = metering_it::clickhouse::get_handle(clickhouse_port)
        .await
        .expect("Could not get clickhouse handle");

    // list all tables in db meteroid
    let tables = clickhouse_client
        .query("SHOW TABLES")
        .fetch_all()
        .await
        .expect("Could not list tables");
    let expected_table_name = metering::connectors::clickhouse::sql::get_meter_view_name(
        &tenant_id,
        &created_metric.billable_metric.unwrap().id,
    )
    .split(".")
    .collect::<Vec<&str>>()[1]
        .to_string();

    //sleep

    tables
        .rows()
        .find(|row| row.get::<String, _>(0).unwrap() == expected_table_name)
        .expect("Could not find meter table");

    // check that events were ingested
    let _events = clickhouse_client
        .query("SELECT * FROM raw_events")
        .fetch_all()
        .await
        .expect("Could not query events");

    // we create a plan
    let plan = meteroid_clients
        .plans
        .create_draft_plan(Request::new(
            meteroid_grpc::meteroid::api::plans::v1::CreateDraftPlanRequest {
                name: "Meteroid AI".to_string(),
                external_id: "meteroid_ai".to_string(),
                description: None,
                product_family_external_id: "default".to_string(),
                plan_type: PlanType::Standard as i32,
            },
        ))
        .await
        .unwrap();

    let plan = plan.into_inner().plan.unwrap();
    let plan_version = plan.current_version.unwrap();
    let plan = plan.plan.unwrap();

    let plan_version_id = plan_version.id;

    let price_component = meteroid_clients
        .price_components
        .clone()
        .create_price_component(tonic::Request::new(
            api::components::v1::CreatePriceComponentRequest {
                plan_version_id: plan_version_id.clone(),
                name: "Capacity".to_string(),
                fee: Some(api::components::v1::Fee {
                    fee_type: Some(api::components::v1::fee::FeeType::Capacity(
                        api::components::v1::fee::CapacityFee {
                            metric_id: metric_id.to_string(),
                            thresholds: vec![
                                api::components::v1::fee::capacity_fee::CapacityThreshold {
                                    included_amount: 100,
                                    price: Decimal::new(1200, 2).to_string(),
                                    per_unit_overage: Decimal::new(5, 2).to_string(),
                                },
                                api::components::v1::fee::capacity_fee::CapacityThreshold {
                                    included_amount: 1000,
                                    price: Decimal::new(8200, 2).to_string(),
                                    per_unit_overage: Decimal::new(4, 2).to_string(),
                                },
                            ],
                        },
                    )),
                }),

                product_item_id: None,
            },
        ))
        .await
        .unwrap()
        .into_inner()
        .component
        .unwrap();

    meteroid_clients
        .plans
        .publish_plan_version(Request::new(api::plans::v1::PublishPlanVersionRequest {
            plan_version_id: plan_version_id.clone(),
            plan_id: plan.id.clone(), // TODO drop ?
        }))
        .await
        .unwrap();

    // we create a subscription
    let subscription = meteroid_clients
        .subscriptions
        .create_subscription(Request::new(
            api::subscriptions::v1::CreateSubscriptionRequest {
                subscription: Some(
                    api::subscriptions::v1::CreateSubscription {
                        plan_version_id: plan_version_id.clone(),
                        billing_start_date: period_1_start.date_naive().to_string(),
                        billing_end_date: None,
                        net_terms: 0,
                        invoice_memo: None,
                        invoice_threshold: None,
                        billing_day,
                        customer_id: customer_1.clone(),
                        currency: "USD".to_string(),
                        trial_start_date: None,
                        components: Some(api::subscriptions::v1::CreateSubscriptionComponents {
                            parameterized_components: vec![
                                api::subscriptions::v1::create_subscription_components::ComponentParameterization {
                                    component_id: price_component.id.clone(),
                                    initial_slot_count: Some(100),
                                    billing_period: None,
                                    committed_capacity: None,
                                }
                            ],
                            overridden_components: vec![],
                            extra_components: vec![],
                            remove_components: vec![],
                        }),
                    },
                )
            },
        ))
        .await
        .unwrap();

    let subscription = subscription.into_inner().subscription.unwrap();

    let _dbg_start_date = chrono_to_date(period_1_start.date_naive()).unwrap();
    let _dbg_end_date = chrono_to_date(period_2_start.date_naive()).unwrap();

    let _invoice_p2 = store
        .insert_invoice(InvoiceNew {
            status: InvoiceStatusEnum::Draft,
            external_status: None,
            tenant_id: tenant_uuid,
            customer_id: Uuid::from_str(&customer_1).unwrap(),
            subscription_id: Some(Uuid::from_str(&subscription.id).unwrap()),
            currency: subscription.currency.clone(),
            days_until_due: Some(subscription.net_terms as i32),
            external_invoice_id: None,
            invoice_id: None,
            invoicing_provider: InvoicingProviderEnum::Stripe,
            line_items: serde_json::Value::Null,
            issued: false,
            issue_attempts: 0,
            last_issue_attempt_at: None,
            last_issue_error: None,
            data_updated_at: None,
            invoice_date: period_2_start.date_naive(),
            amount_cents: Some(100),
            plan_version_id: Some(Uuid::from_str(&plan_version_id).unwrap()),
            invoice_type: InvoiceType::Recurring,
            finalized_at: None,
        })
        .await
        .unwrap();

    let db_invoices = fetch_invoices(&store, tenant_uuid.clone()).await;

    assert_eq!(db_invoices.len(), 2);
    assert_eq!(
        db_invoices
            .into_iter()
            .map(|i| i.status)
            .collect::<Vec<_>>(),
        vec![InvoiceStatusEnum::Finalized, InvoiceStatusEnum::Draft,]
    );

    // DRAFT WORKER
    meteroid::workers::invoicing::draft_worker::draft_worker(&store, now.date_naive())
        .await
        .unwrap();

    let db_invoices = &fetch_invoices(&store, tenant_uuid.clone()).await;

    assert_eq!(db_invoices.len(), 3);
    assert_eq!(
        db_invoices
            .into_iter()
            .map(|i| i.status)
            .collect::<Vec<_>>(),
        vec![
            InvoiceStatusEnum::Finalized,
            InvoiceStatusEnum::Draft,
            InvoiceStatusEnum::Draft,
        ]
    );

    let invoice_p1 = db_invoices.get(0).unwrap();
    let invoice_p2 = db_invoices.get(1).unwrap();
    let invoice_p3 = db_invoices.get(2).unwrap();

    assert_eq!(invoice_p1.invoice_date, period_1_start.date_naive());
    assert_eq!(invoice_p2.invoice_date, period_2_start.date_naive());
    assert_eq!(invoice_p3.invoice_date, period_2_end.date_naive());

    let metering_client = meteroid::workers::clients::metering::MeteringClient::from_channel(
        metering_setup.channel.clone(),
        &metering_config.internal_auth,
    );

    // PRICE WORKER
    meteroid::workers::invoicing::price_worker::price_worker(&store, metering_client.clone())
        .await
        .unwrap();

    let invoice_p2 = store
        .find_invoice_by_id(invoice_p2.tenant_id, invoice_p2.id)
        .await
        .unwrap();

    assert_eq!(invoice_p2.invoice_date, period_2_start.date_naive());

    let invoice_lines: Vec<InvoiceLine> =
        serde_json::from_value(invoice_p2.line_items.clone()).unwrap();
    assert_eq!(invoice_lines.len(), 2);

    let invoice_line = invoice_lines.get(0).unwrap();
    assert_eq!(invoice_line.total, 1200);
    assert_eq!(invoice_line.quantity, Some(1));
    assert_eq!(
        invoice_line.period,
        Some(InvoiceLinePeriod {
            from: period_2_start.date_naive(),
            to: period_2_end.date_naive(),
        })
    );

    let invoice_line = invoice_lines.get(1).unwrap();
    assert_eq!(invoice_line.quantity, Some(149));
    assert_eq!(invoice_line.unit_price, Some(5.0));
    assert_eq!(invoice_line.total, 745);
    assert_eq!(
        invoice_line.period,
        Some(InvoiceLinePeriod {
            from: period_1_start.date_naive(),
            to: period_1_end.date_naive(),
        })
    );

    meteroid::workers::invoicing::pending_status_worker::pending_worker(
        &store,
        chrono::Utc::now().naive_utc(),
    )
    .await
    .unwrap();

    let db_invoices = fetch_invoices(&store, tenant_uuid.clone()).await;
    assert_eq!(
        db_invoices
            .into_iter()
            .map(|i| i.status)
            .collect::<Vec<_>>(),
        vec![
            InvoiceStatusEnum::Finalized,
            InvoiceStatusEnum::Draft, // the invoice is ready to be finalized, so it is not picked up by the pending worker. TODO drop that rule ?
            InvoiceStatusEnum::Draft,
        ]
    );

    // FINALIZER
    meteroid::workers::invoicing::finalize_worker::finalize_worker(&store, metering_client.clone())
        .await
        .unwrap();

    let db_invoices = fetch_invoices(&store, tenant_uuid.clone()).await;
    assert_eq!(
        db_invoices
            .into_iter()
            .map(|i| i.status)
            .collect::<Vec<_>>(),
        vec![
            InvoiceStatusEnum::Finalized,
            InvoiceStatusEnum::Finalized,
            InvoiceStatusEnum::Draft,
        ]
    );

    // ISSUE
    // TODO mock stripe or use a test account

    meteroid_it::container::terminate_meteroid(meteroid_setup.token, meteroid_setup.join_handle)
        .await;
    metering_it::container::terminate_metering(metering_setup.token, metering_setup.join_handle)
        .await;
}

async fn fetch_invoices(store: &Store, tenant_id: Uuid) -> Vec<Invoice> {
    store
        .list_invoices(
            tenant_id,
            None,
            None,
            None,
            OrderByRequest::DateAsc,
            PaginationRequest {
                per_page: Some(100),
                page: 0,
            },
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|x| x.invoice)
        .collect()
}
