\set ON_ERROR_STOP on

DROP DATABASE IF EXISTS codd_dev WITH (FORCE);
CREATE DATABASE codd_dev;

\connect codd_dev

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE SCHEMA analytics;

CREATE TYPE public.customer_tier AS ENUM ('free', 'pro', 'enterprise');
CREATE TYPE public.order_status AS ENUM ('draft', 'processing', 'paid', 'shipped', 'refunded', 'cancelled');

CREATE TABLE public.customers (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    name text NOT NULL,
    email text NOT NULL UNIQUE,
    tier public.customer_tier NOT NULL DEFAULT 'free',
    active boolean NOT NULL DEFAULT true,
    signup_date date NOT NULL,
    preferred_contact_time time,
    profile jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE public.orders (
    id integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id uuid NOT NULL DEFAULT gen_random_uuid() UNIQUE,
    customer_id integer NOT NULL REFERENCES public.customers(id),
    status public.order_status NOT NULL,
    total_amount double precision NOT NULL,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    placed_at timestamp NOT NULL,
    shipped_at timestamptz
);

CREATE TABLE public.audit_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_id uuid NOT NULL DEFAULT gen_random_uuid(),
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
    (public_id, name, email, tier, active, signup_date, preferred_contact_time, profile, created_at)
VALUES
    (
        '8e4af729-4fc2-4c04-8b5a-8d31bc3ef9b1',
        'Ada Lovelace',
        'ada@example.test',
        'pro',
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
        '7a6ad9fd-e2c7-4a53-a580-2fa75d9a2579',
        'Grace Hopper',
        'grace@example.test',
        'enterprise',
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
        '4d9d61d2-9cc3-4288-aaf4-840f0dddf4b8',
        'Katherine Johnson',
        'katherine@example.test',
        'free',
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
    ),
    (
        '9fd07d5a-5a43-4afd-96f7-7b7f52a332c8',
        'Dorothy Vaughan',
        'dorothy@example.test',
        'enterprise',
        true,
        '2025-04-02',
        '10:15',
        '{
            "plan": "enterprise",
            "tags": ["leadership", "fortran", "operations"],
            "team": { "name": "West Area Computers", "size": 32 }
        }',
        '2025-04-02 10:19:33+00'
    ),
    (
        '40bf09f3-9797-49e4-9f66-bf7212851596',
        'Margaret Hamilton',
        'margaret@example.test',
        'pro',
        true,
        '2025-05-18',
        '08:45',
        '{
            "plan": "pro",
            "tags": ["apollo", "software", "reliability"],
            "preferences": { "dark_mode": true, "weekly_digest": true }
        }',
        '2025-05-18 08:51:07+00'
    ),
    (
        '6f77050a-3482-402d-8b44-394e9718f6c6',
        'Hedy Lamarr',
        'hedy@example.test',
        'free',
        false,
        '2025-06-09',
        NULL,
        '{
            "plan": "free",
            "tags": ["radio", "frequency-hopping"],
            "preferences": { "dark_mode": false }
        }',
        '2025-06-09 17:24:18+00'
    ),
    (
        'b1af97db-cb38-4f7f-ac3c-2fc0d99f1e06',
        'Radia Perlman',
        'radia@example.test',
        'pro',
        true,
        '2025-07-14',
        '13:20',
        '{
            "plan": "pro",
            "tags": ["networking", "spanning-tree"],
            "limits": { "projects": 42, "seats": 8 }
        }',
        '2025-07-14 13:22:45+00'
    ),
    (
        '2bb64238-c6b3-489f-b5a0-4c0d72d81ea8',
        'Evelyn Boyd Granville',
        'evelyn@example.test',
        'enterprise',
        true,
        '2025-08-28',
        '11:00',
        '{
            "plan": "enterprise",
            "tags": ["orbit", "mathematics", "education"],
            "account": { "region": "us-east", "owner": "research" }
        }',
        '2025-08-28 11:08:12+00'
    );

INSERT INTO public.orders
    (public_id, customer_id, status, total_amount, metadata, placed_at, shipped_at)
VALUES
    (
        '5f008015-5571-4c7c-973f-8a8fe079f0ad',
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
        '40b25c21-a6e5-42cf-9f07-6ebfb66f5b78',
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
        'a22c8d14-fd2a-4d41-8bb0-3efc4ec118ef',
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
    ),
    (
        '33b42024-2e78-42e3-91f1-773ee0f46988',
        4,
        'shipped',
        849.00,
        '{
            "items": [
                { "sku": "enterprise-seat", "quantity": 8, "price": 100.00 },
                { "sku": "migration-session", "quantity": 1, "price": 49.00 }
            ],
            "delivery": { "carrier": "dhl", "tracking": "DEMO-849-4" }
        }',
        '2026-04-22 13:40:00',
        '2026-04-23 09:15:00+00'
    ),
    (
        '24a856c3-5c57-44b3-a6b0-b2e5ffdfae9d',
        5,
        'paid',
        199.00,
        '{
            "items": [
                { "sku": "sql-client-pro", "quantity": 2, "price": 99.50 }
            ],
            "payment": { "provider": "bank_transfer", "reference": "INV-2026-1004" }
        }',
        '2026-04-23 15:11:12',
        NULL
    ),
    (
        '221af9b1-e698-4994-a9cd-4826022f6ae0',
        6,
        'cancelled',
        59.00,
        '{
            "items": [
                { "sku": "starter-quarterly", "quantity": 1, "price": 59.00 }
            ],
            "cancellation": { "reason": "duplicate_order" }
        }',
        '2026-04-24 08:02:00',
        NULL
    ),
    (
        '886ecdaa-a2b9-4081-8e73-dc1a34fbad08',
        7,
        'draft',
        329.75,
        '{
            "items": [
                { "sku": "team-seat", "quantity": 5, "price": 49.95 },
                { "sku": "query-history-pack", "quantity": 1, "price": 80.00 }
            ],
            "draft": { "expires_at": "2026-05-01T00:00:00Z" }
        }',
        '2026-04-25 19:44:10',
        NULL
    ),
    (
        'd6088c45-47bf-4519-b222-31821a913de1',
        8,
        'processing',
        5100.00,
        '{
            "items": [
                { "sku": "enterprise-seat", "quantity": 48, "price": 100.00 },
                { "sku": "onboarding", "quantity": 1, "price": 300.00 }
            ],
            "approval": { "required": true, "approved_by": "ops@example.test" }
        }',
        '2026-04-26 12:30:00',
        NULL
    );

INSERT INTO public.audit_events
    (request_id, event_type, actor, payload, occurred_at)
VALUES
    (
        '47d9736f-7e4c-45d7-a88d-20c1f357491f',
        'connection.created',
        'admin',
        '{
            "connection": "local demo",
            "host": "localhost",
            "database": "codd_dev"
        }',
        '2026-04-22 09:00:00+00'
    ),
    (
        'cb3c03c8-df21-4f15-bb87-bad235c452af',
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
        '2f77e964-c68b-4d12-b9fd-9fcaed580be5',
        'error.raised',
        NULL,
        '{
            "message": "permission denied for relation secret_table",
            "sql_state": "42501",
            "position": 15
        }',
        '2026-04-22 09:07:51+00'
    ),
    (
        'eb56b69f-1729-4dc1-b24e-9d93d9fb473f',
        'cell.updated',
        'grace@example.test',
        '{
            "table": "public.customers",
            "column": "tier",
            "old_value": "pro",
            "new_value": "enterprise"
        }',
        '2026-04-22 09:11:09+00'
    ),
    (
        '52ea9e87-8ff3-48bd-ab4f-b319b58cc7a0',
        'table.page_loaded',
        'radia@example.test',
        '{
            "schema": "public",
            "table": "orders",
            "page_size": 100,
            "offset": 0
        }',
        '2026-04-22 09:14:27+00'
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

INSERT INTO analytics.page_views
    (path, referrer, session_data, viewed_at)
SELECT
    page.path,
    page.referrer,
    jsonb_build_object(
        'browser', page.browser,
        'screen', jsonb_build_object('width', page.screen_width, 'height', page.screen_height),
        'campaign', page.campaign,
        'session_uuid', gen_random_uuid(),
        'sample_index', series.index
    ),
    '2026-04-23 08:00:00+00'::timestamptz + (series.index * interval '11 minutes')
FROM generate_series(1, 72) AS series(index)
CROSS JOIN LATERAL (
    SELECT
        (ARRAY['/connections', '/editor', '/results', '/history', '/browser', '/settings'])[(series.index % 6) + 1] AS path,
        (ARRAY[NULL::text, '/connections', '/editor', '/browser'])[(series.index % 4) + 1] AS referrer,
        (ARRAY['GNOME Web', 'Firefox', 'Chromium'])[(series.index % 3) + 1] AS browser,
        (ARRAY[1366, 1440, 1920, 2560])[(series.index % 4) + 1] AS screen_width,
        (ARRAY[768, 900, 1080, 1440])[(series.index % 4) + 1] AS screen_height,
        (ARRAY['organic', 'docs', 'release-notes', 'word-of-mouth'])[(series.index % 4) + 1] AS campaign
) AS page;

INSERT INTO public.audit_events
    (request_id, event_type, actor, payload, occurred_at)
SELECT
    gen_random_uuid(),
    (ARRAY['query.executed', 'table.page_loaded', 'cell.updated', 'connection.tested'])[(series.index % 4) + 1],
    (ARRAY['ada@example.test', 'grace@example.test', 'radia@example.test', NULL::text])[(series.index % 4) + 1],
    jsonb_build_object(
        'duration_ms', 5 + (series.index * 7) % 240,
        'row_count', (series.index * 13) % 500,
        'request_index', series.index,
        'demo', true
    ),
    '2026-04-23 09:00:00+00'::timestamptz + (series.index * interval '7 minutes')
FROM generate_series(1, 36) AS series(index);

CREATE VIEW public.active_customers AS
SELECT
    id,
    public_id,
    name,
    email,
    tier,
    signup_date,
    profile ->> 'plan' AS plan,
    created_at
FROM public.customers
WHERE active = true;

CREATE VIEW public.order_summary AS
SELECT
    o.id,
    o.public_id,
    c.name AS customer_name,
    c.tier AS customer_tier,
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
    request_id,
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
CREATE INDEX customers_tier_idx ON public.customers(tier);
CREATE INDEX orders_customer_id_idx ON public.orders(customer_id);
CREATE INDEX orders_status_idx ON public.orders(status);
CREATE INDEX audit_events_payload_gin_idx ON public.audit_events USING gin(payload);
CREATE INDEX audit_events_request_id_idx ON public.audit_events(request_id);
CREATE INDEX page_views_session_data_gin_idx ON analytics.page_views USING gin(session_data);
