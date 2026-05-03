\set ON_ERROR_STOP on

DROP DATABASE IF EXISTS sql_explorer_demo WITH (FORCE);
CREATE DATABASE sql_explorer_demo;

\connect sql_explorer_demo

CREATE SCHEMA analytics;

CREATE TABLE public.customers (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL,
    email text NOT NULL UNIQUE,
    active boolean NOT NULL DEFAULT true,
    signup_date date NOT NULL,
    preferred_contact_time time,
    profile jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE public.orders (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    customer_id integer NOT NULL REFERENCES public.customers(id),
    status text NOT NULL,
    total_amount double precision NOT NULL,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    placed_at timestamp NOT NULL,
    shipped_at timestamptz
);

CREATE TABLE public.audit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    event_type text NOT NULL,
    actor text,
    payload jsonb NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE analytics.page_views (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    path text NOT NULL,
    referrer text,
    session_data jsonb NOT NULL,
    viewed_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO public.customers
    (name, email, active, signup_date, preferred_contact_time, profile, created_at)
VALUES
    (
        'Ada Lovelace',
        'ada@example.test',
        true,
        '2025-01-12',
        '09:30',
        '{
            "plan": "pro",
            "tags": ["founder", "analytics"],
            "address": {
                "city": "London",
                "country": "GB"
            },
            "preferences": {
                "dark_mode": true,
                "weekly_digest": false
            }
        }',
        '2025-01-12 09:34:12+00'
    ),
    (
        'Grace Hopper',
        'grace@example.test',
        true,
        '2025-02-03',
        '14:00',
        '{
            "plan": "enterprise",
            "tags": ["compiler", "navy", "priority"],
            "limits": {
                "seats": 250,
                "projects": 1200
            },
            "notes": "This field is intentionally long enough to test compact JSONB previews in the result grid. Double-click the cell to inspect the full value in the detail window."
        }',
        '2025-02-03 14:18:44+00'
    ),
    (
        'Katherine Johnson',
        'katherine@example.test',
        false,
        '2025-03-21',
        NULL,
        '{
            "plan": "starter",
            "tags": ["space", "math"],
            "preferences": {
                "dark_mode": false
            }
        }',
        '2025-03-21 18:02:09+00'
    );

INSERT INTO public.orders
    (customer_id, status, total_amount, metadata, placed_at, shipped_at)
VALUES
    (
        1,
        'paid',
        129.95,
        '{
            "items": [
                { "sku": "sql-client-pro", "quantity": 1, "price": 99.95 },
                { "sku": "priority-support", "quantity": 1, "price": 30.00 }
            ],
            "payment": {
                "provider": "stripe",
                "last4": "4242"
            }
        }',
        '2026-04-18 10:12:43',
        '2026-04-19 08:10:00+00'
    ),
    (
        2,
        'processing',
        2400.00,
        '{
            "items": [
                { "sku": "enterprise-seat", "quantity": 24, "price": 100.00 }
            ],
            "approval": {
                "required": true,
                "approved_by": null
            }
        }',
        '2026-04-21 16:45:00',
        NULL
    ),
    (
        3,
        'refunded',
        19.90,
        '{
            "items": [
                { "sku": "starter-monthly", "quantity": 1, "price": 19.90 }
            ],
            "refund": {
                "reason": "customer_request",
                "processed_at": "2026-04-22T11:30:00Z"
            }
        }',
        '2026-04-20 07:05:30',
        NULL
    );

INSERT INTO public.audit_events
    (event_type, actor, payload, occurred_at)
VALUES
    (
        'connection.created',
        'admin',
        '{
            "connection": "local demo",
            "host": "localhost",
            "database": "sql_explorer_demo"
        }',
        '2026-04-22 09:00:00+00'
    ),
    (
        'query.executed',
        'ada@example.test',
        '{
            "sql": "select * from public.orders limit 100",
            "duration_ms": 18,
            "row_count": 3
        }',
        '2026-04-22 09:05:12+00'
    ),
    (
        'error.raised',
        NULL,
        '{
            "message": "permission denied for relation secret_table",
            "sql_state": "42501",
            "position": 15
        }',
        '2026-04-22 09:07:51+00'
    );

INSERT INTO analytics.page_views
    (path, referrer, session_data, viewed_at)
VALUES
    (
        '/connections',
        NULL,
        '{
            "browser": "GNOME Web",
            "screen": { "width": 1440, "height": 900 },
            "flags": ["first_open", "dark_theme"]
        }',
        '2026-04-22 12:10:00+00'
    ),
    (
        '/editor',
        '/connections',
        '{
            "browser": "Firefox",
            "screen": { "width": 2560, "height": 1440 },
            "query_length": 42
        }',
        '2026-04-22 12:12:34+00'
    ),
    (
        '/results',
        '/editor',
        '{
            "browser": "Firefox",
            "screen": { "width": 2560, "height": 1440 },
            "result_rows": 500,
            "truncated": true
        }',
        '2026-04-22 12:13:21+00'
    );

CREATE VIEW public.active_customers AS
SELECT
    id,
    name,
    email,
    signup_date,
    profile ->> 'plan' AS plan,
    created_at
FROM public.customers
WHERE active = true;

CREATE VIEW public.order_summary AS
SELECT
    o.id,
    c.name AS customer_name,
    o.status,
    o.total_amount,
    o.placed_at,
    o.shipped_at,
    o.metadata
FROM public.orders o
JOIN public.customers c ON c.id = o.customer_id;

CREATE VIEW public.recent_audit_events AS
SELECT
    id,
    event_type,
    actor,
    payload,
    occurred_at
FROM public.audit_events
ORDER BY occurred_at DESC;

CREATE VIEW analytics.daily_page_views AS
SELECT
    viewed_at::date AS day,
    path,
    count(*)::integer AS views
FROM analytics.page_views
GROUP BY viewed_at::date, path
ORDER BY day DESC, path;

CREATE INDEX customers_active_idx ON public.customers(active);
CREATE INDEX orders_customer_id_idx ON public.orders(customer_id);
CREATE INDEX audit_events_payload_gin_idx ON public.audit_events USING gin(payload);
CREATE INDEX page_views_session_data_gin_idx ON analytics.page_views USING gin(session_data);
