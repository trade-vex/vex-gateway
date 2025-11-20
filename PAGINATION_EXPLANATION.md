# Pagination Explained

## What is Pagination?

Pagination is like reading a book one page at a time instead of trying to read the entire book at once. When you have a large list of items (like 1000 orders), pagination lets you view them in smaller chunks (like 20 at a time).

## Key Concepts

### 1. **Page Number** (`page`)
- Which "page" of results you want to see
- Starts at 1 (not 0)
- Example: `page=1` is the first page, `page=2` is the second page

### 2. **Limit** (`limit`)
- How many items per page
- Example: `limit=20` means 20 items per page

### 3. **Offset**
- How many items to skip before starting
- Calculated as: `offset = (page - 1) × limit`
- This is what the database uses to know where to start

## Visual Example

Imagine you have **45 orders total** and want **10 orders per page**:

```
All Orders (45 total):
[1] [2] [3] [4] [5] [6] [7] [8] [9] [10] [11] [12] [13] [14] [15] [16] [17] [18] [19] [20] 
[21] [22] [23] [24] [25] [26] [27] [28] [29] [30] [31] [32] [33] [34] [35] [36] [37] [38] [39] [40]
[41] [42] [43] [44] [45]
```

**Page 1** (`page=1, limit=10`):
- Offset = (1-1) × 10 = 0 (skip 0 items)
- Returns: Orders 1-10
- Response: `{orders: [1-10], page: 1, total: 45, total_pages: 5, has_next: true, has_prev: false}`

**Page 2** (`page=2, limit=10`):
- Offset = (2-1) × 10 = 10 (skip first 10 items)
- Returns: Orders 11-20
- Response: `{orders: [11-20], page: 2, total: 45, total_pages: 5, has_next: true, has_prev: true}`

**Page 3** (`page=3, limit=10`):
- Offset = (3-1) × 10 = 20 (skip first 20 items)
- Returns: Orders 21-30
- Response: `{orders: [21-30], page: 3, total: 45, total_pages: 5, has_next: true, has_prev: true}`

**Page 5** (`page=5, limit=10`):
- Offset = (5-1) × 10 = 40 (skip first 40 items)
- Returns: Orders 41-45 (only 5 items)
- Response: `{orders: [41-45], page: 5, total: 45, total_pages: 5, has_next: false, has_prev: true}`

## How It Works in Our Code

### Step 1: Client Makes Request
```bash
GET /api/user/orders/open?page=2&limit=10
```

### Step 2: Server Parses Parameters
```rust
let page = query.page.unwrap_or(1).max(1);        // page = 2
let limit = query.limit.unwrap_or(20).min(100);    // limit = 10
let offset = ((page - 1) * limit) as u64;         // offset = (2-1) × 10 = 10
```

### Step 3: Database Query
```rust
// In repository.rs
OrderEntity::find()
    .filter(OrderColumn::OrderUserId.eq(user_id))
    .order_by_desc(OrderColumn::OrderTimestamp)
    .offset(10)    // Skip first 10 records
    .limit(10)     // Take next 10 records
    .all(&*self.db)
```

**SQL Equivalent:**
```sql
SELECT * FROM orders 
WHERE user_id = 123 
ORDER BY timestamp DESC 
OFFSET 10 LIMIT 10;
```

This means: "Skip the first 10 rows, then return the next 10 rows"

### Step 4: Calculate Metadata
```rust
let total_count = 45;  // Total number of orders
let total_pages = (total_count + limit - 1) / limit;  // (45 + 10 - 1) / 10 = 5
let has_next = page < total_pages;  // 2 < 5 = true
let has_prev = page > 1;            // 2 > 1 = true
```

### Step 5: Return Response
```json
{
  "orders": [
    // 10 orders here (orders 11-20)
  ],
  "total": 45,
  "page": 2,
  "limit": 10,
  "total_pages": 5,
  "has_next": true,
  "has_prev": true
}
```

## Response Fields Explained

- **`orders`**: The actual data for this page (array of order objects)
- **`total`**: Total number of items across all pages (45)
- **`page`**: Current page number (2)
- **`limit`**: Items per page (10)
- **`total_pages`**: Total number of pages (5)
- **`has_next`**: Can you go to the next page? (true/false)
- **`has_prev`**: Can you go to the previous page? (true/false)

## Real-World Usage Examples

### Example 1: First Page (Default)
```bash
GET /api/user/orders/open
# or explicitly:
GET /api/user/orders/open?page=1&limit=20
```
Returns first 20 orders

### Example 2: Second Page
```bash
GET /api/user/orders/open?page=2&limit=20
```
Returns orders 21-40

### Example 3: Custom Page Size
```bash
GET /api/user/orders/open?page=1&limit=50
```
Returns first 50 orders (if you want more per page)

### Example 4: Last Page
```bash
GET /api/user/orders/open?page=5&limit=20
```
If there are 90 orders total, this returns orders 81-90

## Benefits

1. **Performance**: Only loads 20 items instead of 1000
2. **Speed**: Faster API responses
3. **User Experience**: Users can navigate through pages
4. **Network**: Less data transferred
5. **Memory**: Server uses less memory

## Common Patterns

### Frontend Implementation
```javascript
// Load first page
const response = await fetch('/api/user/orders/open?page=1&limit=20');
const data = await response.json();

// Show orders
displayOrders(data.orders);

// Enable/disable next button
nextButton.disabled = !data.has_next;

// Enable/disable prev button
prevButton.disabled = !data.has_prev;

// Go to next page
if (data.has_next) {
  const nextPage = data.page + 1;
  const nextResponse = await fetch(`/api/user/orders/open?page=${nextPage}&limit=20`);
  // ...
}
```

## Edge Cases Handled

1. **Invalid page number**: Defaults to 1, minimum is 1
2. **Invalid limit**: Defaults to 20, maximum is 100, minimum is 1
3. **Empty results**: Returns empty array with correct metadata
4. **Last page with fewer items**: Returns remaining items (e.g., 5 items on last page)

## Summary

Pagination = Breaking large lists into smaller, manageable pages

- **Page**: Which chunk you want
- **Limit**: How big each chunk is
- **Offset**: How many items to skip (calculated automatically)
- **Metadata**: Information about total pages, next/prev availability

This makes APIs faster, more efficient, and provides better user experience!

