-- PhazeAI Telemetry Schema
-- Run this in Supabase Dashboard > SQL Editor

-- Anonymous usage pings (one per app launch)
create table if not exists telemetry (
  id bigint generated always as identity primary key,
  app text not null check (app in ('ide', 'cli')),
  version text not null,
  os text not null,
  arch text not null,
  session_id uuid not null,
  created_at timestamptz not null default now()
);

-- Index for fast counting
create index if not exists idx_telemetry_app on telemetry (app);
create index if not exists idx_telemetry_created on telemetry (created_at);

-- Unique active installs (deduplicated by day)
create or replace view daily_active_users as
select
  date_trunc('day', created_at) as day,
  app,
  count(distinct session_id) as unique_sessions,
  count(*) as total_launches
from telemetry
group by 1, 2
order by 1 desc;

-- Total counts view
create or replace view usage_summary as
select
  app,
  count(*) as total_launches,
  count(distinct session_id) as unique_installs,
  count(distinct date_trunc('day', created_at)) as active_days,
  min(created_at) as first_seen,
  max(created_at) as last_seen
from telemetry
group by app;

-- Allow anonymous inserts (the anon key can write telemetry)
alter table telemetry enable row level security;

create policy "Allow anonymous inserts"
  on telemetry for insert
  to anon
  with check (true);

-- Allow reading aggregate views (for a future public stats page)
create policy "Allow anonymous reads"
  on telemetry for select
  to anon
  using (true);
