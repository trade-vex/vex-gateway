//! unit tests for dto trading structures
//! 
//! these tests verify that our dto structures work correctly with serialization,
//! deserialization, and conversion logic.

use lib_core::dto::trading::{
    BalancesListResponse, OrderResponse, OrdersListResponse, Side, UserBalanceResponse,
};
use lib_core::model::user_balances::Model as UserBalanceModel;
use lib_core::model::orders::Model as OrderModel;
use chrono::Utc;
use uuid::Uuid;

// helper function to create a test user balance model
fn create_test_user_balance(user_id: Uuid) -> UserBalanceModel {
    UserBalanceModel {
        user_id,
        available: 1000,
        locked: 200,
        last_updated: Utc::now(),
    }
}

// helper function to create a test order model
fn create_test_order(user_id: Uuid) -> OrderModel {
    OrderModel {
        order_id: 12345,
        user_id,
        price: 500,
        size: 10,
        side: "Bid".to_string(),
        timestamp: 1234567890,
    }
}

#[test]
fn test_side_enum_serialization() {
    // test that Side enum can be serialized
    let bid_side = Side::Bid;
    let ask_side = Side::Ask;
    
    let bid_json = serde_json::to_string(&bid_side).unwrap();
    let ask_json = serde_json::to_string(&ask_side).unwrap();
    
    assert_eq!(bid_json, "\"Bid\"");
    assert_eq!(ask_json, "\"Ask\"");
}

#[test]
fn test_side_enum_deserialization() {
    // test that Side enum can be deserialized
    let bid_json = "\"Bid\"";
    let ask_json = "\"Ask\"";
    
    let bid_side: Side = serde_json::from_str(bid_json).unwrap();
    let ask_side: Side = serde_json::from_str(ask_json).unwrap();
    
    assert_eq!(bid_side, Side::Bid);
    assert_eq!(ask_side, Side::Ask);
}

#[test]
fn test_side_enum_display() {
    // test that Side enum implements Display correctly
    let bid_side = Side::Bid;
    let ask_side = Side::Ask;
    
    assert_eq!(bid_side.to_string(), "Bid");
    assert_eq!(ask_side.to_string(), "Ask");
}

#[test]
fn test_side_enum_from_str() {
    // test that Side enum can be parsed from string
    let bid_side: Side = "Bid".parse().unwrap();
    let ask_side: Side = "Ask".parse().unwrap();
    
    assert_eq!(bid_side, Side::Bid);
    assert_eq!(ask_side, Side::Ask);
    
    // test invalid string parsing
    let invalid_result: Result<Side, _> = "InvalidSide".parse();
    assert!(invalid_result.is_err());
}

#[test]
fn test_order_response_from_model() {
    // test that OrderModel converts to OrderResponse correctly
    let user_id = Uuid::new_v4();
    let order_model = create_test_order(user_id);
    
    let order_response: OrderResponse = order_model.into();
    
    assert_eq!(order_response.order_id, 12345);
    assert_eq!(order_response.user_id, user_id);
    assert_eq!(order_response.price, 500);
    assert_eq!(order_response.size, 10);
    assert_eq!(order_response.side, Side::Bid);
    assert_eq!(order_response.timestamp, 1234567890);
}

#[test]
fn test_user_balance_response_from_model() {
    // test that UserBalanceModel converts to UserBalanceResponse correctly
    let user_id = Uuid::new_v4();
    let balance_model = create_test_user_balance(user_id);
    
    let balance_response: UserBalanceResponse = balance_model.into();
    
    assert_eq!(balance_response.user_id, user_id);
    assert_eq!(balance_response.available, 1000);
    assert_eq!(balance_response.locked, 200);
    assert_eq!(balance_response.total, 1200); // available + locked
}

#[test]
fn test_balances_list_response_structure() {
    // test that BalancesListResponse can be serialized and deserialized
    let user_id = Uuid::new_v4();
    let balance = create_test_user_balance(user_id);
    let balance_response: UserBalanceResponse = balance.into();
    
    let response = BalancesListResponse {
        balances: vec![balance_response],
        total_assets: 1,
    };
    
    let json = serde_json::to_string(&response).unwrap();
    let deserialized: BalancesListResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(response.balances.len(), deserialized.balances.len());
    assert_eq!(response.total_assets, deserialized.total_assets);
}

#[test]
fn test_orders_list_response_structure() {
    // test that OrdersListResponse can be serialized and deserialized
    let user_id = Uuid::new_v4();
    let order = create_test_order(user_id);
    let order_response: OrderResponse = order.into();
    
    let response = OrdersListResponse {
        orders: vec![order_response],
        total: 1,
    };
    
    let json = serde_json::to_string(&response).unwrap();
    let deserialized: OrdersListResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(response.orders.len(), deserialized.orders.len());
    assert_eq!(response.total, deserialized.total);
}

#[test]
fn test_dto_json_serialization() {
    // test comprehensive JSON serialization of all DTOs
    let user_id = Uuid::new_v4();
    
    // test OrderResponse serialization
    let order_response = OrderResponse {
        order_id: 12345,
        user_id,
        price: 1000,
        size: 5,
        side: Side::Bid,
        timestamp: 1234567890,
    };
    
    let order_json = serde_json::to_string(&order_response).unwrap();
    let deserialized_order: OrderResponse = serde_json::from_str(&order_json).unwrap();
    
    assert_eq!(order_response.order_id, deserialized_order.order_id);
    assert_eq!(order_response.user_id, deserialized_order.user_id);
    assert_eq!(order_response.price, deserialized_order.price);
    assert_eq!(order_response.size, deserialized_order.size);
    assert_eq!(order_response.side, deserialized_order.side);
    assert_eq!(order_response.timestamp, deserialized_order.timestamp);
    
    // test UserBalanceResponse serialization
    let balance_response = UserBalanceResponse {
        user_id,
        available: 2000,
        locked: 500,
        total: 2500,
        last_updated: Utc::now(),
    };
    
    let balance_json = serde_json::to_string(&balance_response).unwrap();
    let deserialized_balance: UserBalanceResponse = serde_json::from_str(&balance_json).unwrap();
    
    assert_eq!(balance_response.user_id, deserialized_balance.user_id);
    assert_eq!(balance_response.available, deserialized_balance.available);
    assert_eq!(balance_response.locked, deserialized_balance.locked);
    assert_eq!(balance_response.total, deserialized_balance.total);
}

#[test]
fn test_dto_json_deserialization() {
    // test comprehensive JSON deserialization of all DTOs
    let _user_id = Uuid::new_v4();
    
    // test OrderResponse deserialization
    let order_json = r#"{
        "order_id": 54321,
        "user_id": "550e8400-e29b-41d4-a716-446655440000",
        "price": 2000,
        "size": 15,
        "side": "Ask",
        "timestamp": 9876543210
    }"#;
    
    let order_response: OrderResponse = serde_json::from_str(order_json).unwrap();
    assert_eq!(order_response.order_id, 54321);
    assert_eq!(order_response.price, 2000);
    assert_eq!(order_response.size, 15);
    assert_eq!(order_response.side, Side::Ask);
    assert_eq!(order_response.timestamp, 9876543210);
    
    // test UserBalanceResponse deserialization
    let balance_json = r#"{
        "user_id": "550e8400-e29b-41d4-a716-446655440000",
        "available": 3000,
        "locked": 750,
        "total": 3750,
        "last_updated": "2023-01-01T00:00:00Z"
    }"#;
    
    let balance_response: UserBalanceResponse = serde_json::from_str(balance_json).unwrap();
    assert_eq!(balance_response.available, 3000);
    assert_eq!(balance_response.locked, 750);
    assert_eq!(balance_response.total, 3750);
}

#[test]
fn test_edge_case_side_conversion() {
    // test edge cases for Side enum conversion
    let user_id = Uuid::new_v4();
    
    // test with valid Bid side
    let bid_order = OrderModel {
        order_id: 1,
        user_id,
        price: 100,
        size: 5,
        side: "Bid".to_string(),
        timestamp: 1234567890,
    };
    
    let bid_response: OrderResponse = bid_order.into();
    assert_eq!(bid_response.side, Side::Bid);
    
    // test with valid Ask side
    let ask_order = OrderModel {
        order_id: 2,
        user_id,
        price: 200,
        size: 10,
        side: "Ask".to_string(),
        timestamp: 1234567891,
    };
    
    let ask_response: OrderResponse = ask_order.into();
    assert_eq!(ask_response.side, Side::Ask);
    
    // test with invalid side (should default to Bid)
    let invalid_order = OrderModel {
        order_id: 3,
        user_id,
        price: 300,
        size: 15,
        side: "InvalidSide".to_string(),
        timestamp: 1234567892,
    };
    
    let invalid_response: OrderResponse = invalid_order.into();
    assert_eq!(invalid_response.side, Side::Bid); // should default to Bid
}

#[test]
fn test_edge_case_balance_calculation() {
    // test edge cases for balance total calculation
    let user_id = Uuid::new_v4();
    
    // test with zero values
    let zero_balance = UserBalanceModel {
        user_id,
        available: 0,
        locked: 0,
        last_updated: Utc::now(),
    };
    
    let zero_response: UserBalanceResponse = zero_balance.into();
    assert_eq!(zero_response.total, 0);
    
    // test with large values
    let large_balance = UserBalanceModel {
        user_id,
        available: i64::MAX / 2,
        locked: i64::MAX / 2,
        last_updated: Utc::now(),
    };
    
    let large_response: UserBalanceResponse = large_balance.into();
    assert_eq!(large_response.total, (i64::MAX / 2 + i64::MAX / 2) as u64);
    
    // test with negative values (converted from i64)
    let negative_balance = UserBalanceModel {
        user_id,
        available: -1000,
        locked: -200,
        last_updated: Utc::now(),
    };
    
    let negative_response: UserBalanceResponse = negative_balance.into();
    assert_eq!(negative_response.available, (-1000i64) as u64);
    assert_eq!(negative_response.locked, (-200i64) as u64);
    assert_eq!(negative_response.total, ((-1000i64) + (-200i64)) as u64);
}

#[test]
fn test_edge_case_order_conversion() {
    // test edge cases for order conversion
    let user_id = Uuid::new_v4();
    
    // test with zero values
    let zero_order = OrderModel {
        order_id: 0,
        user_id,
        price: 0,
        size: 0,
        side: "Bid".to_string(),
        timestamp: 0,
    };
    
    let zero_response: OrderResponse = zero_order.into();
    assert_eq!(zero_response.order_id, 0);
    assert_eq!(zero_response.price, 0);
    assert_eq!(zero_response.size, 0);
    assert_eq!(zero_response.timestamp, 0);
    
    // test with maximum values
    let max_order = OrderModel {
        order_id: i64::MAX,
        user_id,
        price: i64::MAX,
        size: i64::MAX,
        side: "Ask".to_string(),
        timestamp: i64::MAX,
    };
    
    let max_response: OrderResponse = max_order.into();
    assert_eq!(max_response.order_id, i64::MAX as u64);
    assert_eq!(max_response.price, i64::MAX as u64);
    assert_eq!(max_response.size, i64::MAX as u64);
    assert_eq!(max_response.timestamp, i64::MAX as u64);
}

#[test]
fn test_response_list_edge_cases() {
    // test edge cases for response list structures
    let user_id = Uuid::new_v4();
    
    // test empty balances list
    let empty_balances = BalancesListResponse {
        balances: vec![],
        total_assets: 0,
    };
    
    let json = serde_json::to_string(&empty_balances).unwrap();
    let deserialized: BalancesListResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(empty_balances.balances.len(), deserialized.balances.len());
    assert_eq!(empty_balances.total_assets, deserialized.total_assets);
    
    // test empty orders list
    let empty_orders = OrdersListResponse {
        orders: vec![],
        total: 0,
    };
    
    let json = serde_json::to_string(&empty_orders).unwrap();
    let deserialized: OrdersListResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(empty_orders.orders.len(), deserialized.orders.len());
    assert_eq!(empty_orders.total, deserialized.total);
    
    // test large lists
    let mut large_balances = Vec::new();
    for i in 0..100 {
        let balance = UserBalanceModel {
            user_id,
            available: i,
            locked: i * 2,
            last_updated: Utc::now(),
        };
        large_balances.push(balance.into());
    }
    
    let large_response = BalancesListResponse {
        balances: large_balances,
        total_assets: 100,
    };
    
    let json = serde_json::to_string(&large_response).unwrap();
    let deserialized: BalancesListResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(large_response.balances.len(), deserialized.balances.len());
    assert_eq!(large_response.total_assets, deserialized.total_assets);
}

#[test]
fn test_uuid_handling() {
    // test that UUIDs are handled correctly in DTOs
    let user_id = Uuid::new_v4();
    
    let balance = UserBalanceModel {
        user_id,
        available: 1000,
        locked: 200,
        last_updated: Utc::now(),
    };
    
    let balance_response: UserBalanceResponse = balance.into();
    
    // test that UUID is preserved correctly
    assert_eq!(balance_response.user_id, user_id);
    
    // test UUID serialization/deserialization
    let json = serde_json::to_string(&balance_response).unwrap();
    let deserialized: UserBalanceResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(balance_response.user_id, deserialized.user_id);
}

#[test]
fn test_timestamp_handling() {
    // test that timestamps are handled correctly
    let user_id = Uuid::new_v4();
    
    // test with current timestamp
    let now = Utc::now();
    let balance = UserBalanceModel {
        user_id,
        available: 1000,
        locked: 200,
        last_updated: now,
    };
    
    let balance_response: UserBalanceResponse = balance.into();
    assert_eq!(balance_response.last_updated, now);
    
    // test timestamp serialization/deserialization
    let json = serde_json::to_string(&balance_response).unwrap();
    let deserialized: UserBalanceResponse = serde_json::from_str(&json).unwrap();
    
    assert_eq!(balance_response.last_updated, deserialized.last_updated);
}

#[test]
fn test_dto_consistency() {
    // test that DTO conversions are consistent
    let user_id = Uuid::new_v4();
    
    // create original models
    let balance = create_test_user_balance(user_id);
    let order = create_test_order(user_id);
    
    // convert to DTOs
    let balance_response: UserBalanceResponse = balance.clone().into();
    let order_response: OrderResponse = order.clone().into();
    
    // convert back to JSON and deserialize
    let balance_json = serde_json::to_string(&balance_response).unwrap();
    let order_json = serde_json::to_string(&order_response).unwrap();
    
    let deserialized_balance: UserBalanceResponse = serde_json::from_str(&balance_json).unwrap();
    let deserialized_order: OrderResponse = serde_json::from_str(&order_json).unwrap();
    
    // verify consistency
    assert_eq!(balance_response.user_id, deserialized_balance.user_id);
    assert_eq!(balance_response.available, deserialized_balance.available);
    assert_eq!(balance_response.locked, deserialized_balance.locked);
    assert_eq!(balance_response.total, deserialized_balance.total);
    
    assert_eq!(order_response.order_id, deserialized_order.order_id);
    assert_eq!(order_response.user_id, deserialized_order.user_id);
    assert_eq!(order_response.price, deserialized_order.price);
    assert_eq!(order_response.size, deserialized_order.size);
    assert_eq!(order_response.side, deserialized_order.side);
    assert_eq!(order_response.timestamp, deserialized_order.timestamp);
}
