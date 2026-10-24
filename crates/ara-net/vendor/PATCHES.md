# Lightyear application-clock patches

## Upstream sources

These are complete `lightyear_sync` **0.31.0** and `lightyear_transport` **0.31.0**
crate sources from crates.io, at upstream commit
[`8a926d6fece6a0cf04c2747de90c01258524fe0d`](https://github.com/cBournhonesque/lightyear/tree/8a926d6fece6a0cf04c2747de90c01258524fe0d).
Their published manifests, lockfiles and provenance metadata are unchanged.
Each directory includes upstream `LICENSE-MIT` and `LICENSE-APACHE`.
Only these two crates are patched; every other Lightyear crate resolves to
0.31.0 from crates.io. The Cargo registry cache marker `.cargo-ok` is not source.

Published crate archive SHA-256 checksums (also checked against the original lockfile):

- `lightyear_sync`: `82eceb35d8eace3b1808e19e27582ed3b669964a335df4abf7723bfe922403e8`
- `lightyear_transport`: `af892d197389fab9d663f81f023b4db0640ca488d5ac2e6cf7b6db36d2edd40d`

## Why

Ara's explicit network frame driver uses one application clock, Bevy
`Time<Real>`, for RTT, timers and bandwidth admission. Direct wall-clock reads
inside Lightyear bypassed that clock: advancing virtual time could leave RTT
measurements and quota refill on physical time. Under load, the old wall-clock
gate could also deliver inputs late and retime their histories, so an exact
out-and-back movement script did not necessarily return to its starting point.

1. `lightyear_sync`: take every ping/pong timestamp from the application's frame
   clock, matching the existing ping timers and timeline synchronization.
2. `lightyear_transport`: advance Governor's relative clock from transport elapsed
   time before packet admission. Preserve Governor's algorithm, byte accounting,
   rate and burst quotas. The focused test checks exact refill boundaries,
   repeated timestamps and backward timestamps.

`ara-net` drives peers and the seeded UDP conditioner with one virtual frame
clock. The gate keeps its position, bandwidth, replay, privacy and rollback
assertions, and checks the authority's neutral input tail before finishing.

## Dropping the patches on upgrade

Upgrade the Lightyear family together. Confirm that upstream uses the same
application clock for RTT timestamps, retransmission/ping timers and quota
refill; passing live wall-clock tests alone does not establish this.
Once upstream covers both changes, remove the two root `[patch.crates-io]`
entries and their comment, delete these vendored sources, and refresh
`Cargo.lock`. Keep the explicit frame driver and its regression coverage.
Run the gate 20 times under build load, the net/runtime-net/player tests,
touched-crate clippy with `-D warnings`, and the workspace formatting check.

## Exact diff against the published crate sources

This is the same source diff as `target/net-gate-determinism/backend-clock.patch`,
with paths relative to this directory.

```diff
diff --git a/lightyear_sync/src/ping/plugin.rs b/lightyear_sync/src/ping/plugin.rs
index 840812f..c64fede 100644
--- a/lightyear_sync/src/ping/plugin.rs
+++ b/lightyear_sync/src/ping/plugin.rs
@@ -11,7 +11,6 @@ use lightyear_connection::client::Connected;
 use lightyear_connection::direction::NetworkDirection;
 use lightyear_connection::host::HostClient;
 use lightyear_core::tick::TickDuration;
-use lightyear_core::time::Instant;
 use lightyear_core::time::TickDelta;
 use lightyear_link::Link;
 use lightyear_messages::plugin::MessageSystems;
@@ -50,6 +49,9 @@ impl PingPlugin {
             (With<Connected>, Without<HostClient>),
         >,
     ) {
+        let Some(now) = real_time.last_update() else {
+            return;
+        };
         adaptive_for_each_mut!(query).for_each(
             |(mut link, mut m, mut ping_receiver, mut pong_receiver)| {
                 // update
@@ -57,12 +59,12 @@ impl PingPlugin {
 
                 // receive pings
                 ping_receiver.receive().for_each(|ping| {
-                    m.buffer_pending_pong(&ping, Instant::now());
+                    m.buffer_pending_pong(&ping, now);
                 });
                 // receive pongs
                 pong_receiver.receive().for_each(|pong| {
                     // process the pong
-                    m.process_pong(&pong, Instant::now(), tick_duration.0);
+                    m.process_pong(&pong, now, tick_duration.0);
                 });
 
                 link.stats.rtt = m.rtt();
@@ -75,6 +77,7 @@ impl PingPlugin {
     /// We modify the pongs that were buffered so that we can write the correct
     /// time spent between PostUpdate and PreUpdate
     fn send(
+        real_time: Res<Time<Real>>,
         tick_duration: Res<TickDuration>,
         mut query: Query<
             (
@@ -86,13 +89,10 @@ impl PingPlugin {
             (With<Connected>, Without<HostClient>),
         >,
     ) {
-        let now = Instant::now();
-        // NOTE: the real_time.last_update() is the time from the Render World! It seems like it cannot be compared directly
-        //  with the time from Instant::now(), so we stick to only using Instant::now() for now.
-        // let Some(frame_start) = real_time.last_update() else {
-        //     return
-        // };
-        // let frame_time = now - frame_start;
+        // Use one clock for RTT samples, timers and timeline synchronization.
+        let Some(now) = real_time.last_update() else {
+            return;
+        };
         adaptive_for_each_mut!(query).for_each(
             |(entity, mut m, mut ping_sender, mut pong_sender)| {
                 // send the pings
diff --git a/lightyear_transport/src/packet/priority_manager.rs b/lightyear_transport/src/packet/priority_manager.rs
index de0f1ed..c8702a5 100644
--- a/lightyear_transport/src/packet/priority_manager.rs
+++ b/lightyear_transport/src/packet/priority_manager.rs
@@ -2,7 +2,13 @@ use crate::channel::registry::ChannelRegistry;
 use crate::packet::message::{MessageData, SendCandidate};
 use alloc::vec::Vec;
 use core::num::NonZeroU32;
-use governor::{DefaultDirectRateLimiter, Quota};
+use core::time::Duration;
+use governor::{
+    Quota, RateLimiter,
+    clock::{Clock, FakeRelativeClock},
+    middleware::NoOpMiddleware,
+    state::{InMemoryState, direct::NotKeyed},
+};
 use lightyear_serde::ToBytes;
 use nonzero_ext::*;
 #[cfg(feature = "trace")]
@@ -66,7 +72,13 @@ pub struct PriorityManager {
 #[derive(Debug)]
 pub(crate) struct BandwidthLimiter {
     enabled: bool,
-    limiter: DefaultDirectRateLimiter,
+    limiter: RateLimiter<
+        NotKeyed,
+        InMemoryState,
+        FakeRelativeClock,
+        NoOpMiddleware<<FakeRelativeClock as Clock>::Instant>,
+    >,
+    updated: Duration,
 }
 
 impl Default for PriorityManager {
@@ -162,7 +174,19 @@ impl BandwidthLimiter {
     pub(crate) fn new(config: PriorityConfig) -> Self {
         Self {
             enabled: config.enabled,
-            limiter: DefaultDirectRateLimiter::direct(config.bandwidth_quota),
+            limiter: RateLimiter::direct_with_clock(
+                config.bandwidth_quota,
+                FakeRelativeClock::default(),
+            ),
+            updated: Duration::ZERO,
+        }
+    }
+
+    /// Refill quota from the same application clock as transport retransmission.
+    pub(crate) fn update(&mut self, elapsed: Duration) {
+        if let Some(delta) = elapsed.checked_sub(self.updated) {
+            self.limiter.clock().advance(delta);
+            self.updated = elapsed;
         }
     }
 
@@ -497,4 +521,21 @@ mod tests {
         assert!(limiter.consume_packet_quota(300));
         assert!(!limiter.consume_packet_quota(301));
     }
+
+    #[test]
+    fn bandwidth_refills_only_from_transport_time() {
+        let mut limiter = BandwidthLimiter::new(PriorityConfig::new(600));
+        assert!(limiter.consume_packet_quota(600));
+        assert!(!limiter.consume_packet_quota(1));
+        limiter.update(Duration::from_millis(250));
+        assert!(limiter.consume_packet_quota(150));
+        assert!(!limiter.consume_packet_quota(1));
+        limiter.update(Duration::from_millis(250));
+        assert!(!limiter.consume_packet_quota(1));
+        limiter.update(Duration::from_millis(100));
+        assert!(!limiter.consume_packet_quota(1));
+        limiter.update(Duration::from_secs(1));
+        assert!(limiter.consume_packet_quota(450));
+        assert!(!limiter.consume_packet_quota(1));
+    }
 }
diff --git a/lightyear_transport/src/plugin.rs b/lightyear_transport/src/plugin.rs
index 9fbef00..b566a72 100644
--- a/lightyear_transport/src/plugin.rs
+++ b/lightyear_transport/src/plugin.rs
@@ -410,6 +410,7 @@ impl TransportPlugin {
         query.for_each(|(mut link, mut transport, host_client)| {
             // allow split borrows
             let transport = &mut *transport;
+            transport.bandwidth_limiter.update(real_time.elapsed());
             let mtu = link.mtu();
 
             // buffer all new messages in the Sender
```
