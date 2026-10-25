//! Exercise the real transport and lifetime rules without requiring a GPU.
use super::{
    client::Remote,
    server::{self, Pool},
    wire,
};
use ara_asset::{BlockEncoder, Cancellation, CookPreset, Error, Result};
use ara_asset_format::TextureFormat;
use std::{
    collections::BTreeMap,
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[derive(Default)]
struct Counts {
    calls: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
}
struct Fake {
    counts: Arc<Counts>,
    initialized: AtomicBool,
}
impl BlockEncoder for Fake {
    fn backend(&self) -> &str {
        "fixture.service"
    }
    fn metrics(&self) -> BTreeMap<String, f64> {
        if self.initialized.load(Ordering::Acquire) {
            BTreeMap::from([("initialize.wall_ms".into(), 1.0)])
        } else {
            BTreeMap::new()
        }
    }
    fn encode(
        &self,
        format: TextureFormat,
        _: CookPreset,
        words: &[u32],
        cancel: &Cancellation,
    ) -> Result<Vec<u8>> {
        cancel.check()?;
        if words[0] == u32::MAX {
            return Err(Error::EncoderUnavailable("fixture failure".into()));
        }
        self.initialized.store(true, Ordering::Release);
        let active = self.counts.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.counts.peak.fetch_max(active, Ordering::AcqRel);
        self.counts.calls.fetch_add(1, Ordering::AcqRel);
        thread::sleep(Duration::from_millis(5));
        self.counts.active.fetch_sub(1, Ordering::AcqRel);
        Ok(vec![
            words[0] as u8;
            words.len() / wire::words(format) * format.block_bytes()
        ])
    }
}
fn fixture() -> Result<(wire::Descriptor, mpsc::Receiver<()>, Arc<Counts>)> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let counts = Arc::new(Counts::default());
    let encoders = (0..2)
        .map(|_| {
            Box::new(Fake {
                counts: Arc::clone(&counts),
                initialized: AtomicBool::new(false),
            }) as Box<dyn BlockEncoder>
        })
        .collect();
    let descriptor = server::descriptor(&listener, "fixture.service".into())?;
    let server_descriptor = descriptor.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        server::serve(
            listener,
            &server_descriptor,
            Arc::new(Pool::new(encoders)),
            Duration::from_millis(30),
        )
        .expect("fixture service");
        tx.send(()).expect("fixture receiver");
    });
    Ok((descriptor, rx, counts))
}

#[test]
fn devices_survive_disposable_clients_but_not_the_host_lease() -> Result<()> {
    let (descriptor, done, counts) = fixture()?;
    let lease = descriptor.endpoint.lease()?;
    for value in 1..=8 {
        let remote = Remote::connect(descriptor.clone())?;
        assert_eq!(
            remote.encode(
                TextureFormat::Bc4,
                CookPreset::Balanced,
                &[value; 16],
                &Cancellation::default()
            )?,
            vec![value as u8; 8]
        );
        assert_eq!(remote.metrics()["service.devices_created"], 1.0);
    }
    assert_eq!(counts.calls.load(Ordering::Acquire), 8);
    assert!(
        done.recv_timeout(Duration::from_millis(100)).is_err(),
        "idle device remains alive with its host"
    );
    drop(lease);
    done.recv_timeout(Duration::from_secs(2))
        .expect("service exits after host lease closes");
    Ok(())
}

#[test]
fn concurrent_clients_share_bounded_slots_and_errors_remain_explicit() -> Result<()> {
    let (descriptor, done, counts) = fixture()?;
    let lease = descriptor.endpoint.lease()?;
    thread::scope(|scope| {
        for _ in 0..8 {
            let descriptor = descriptor.clone();
            scope.spawn(move || {
                let remote = Remote::connect(descriptor).expect("client");
                for _ in 0..4 {
                    assert_eq!(
                        remote
                            .encode(
                                TextureFormat::Bc4,
                                CookPreset::HighQuality,
                                &[9; 16],
                                &Cancellation::default()
                            )
                            .expect("encode"),
                        vec![9; 8]
                    );
                }
                assert!(matches!(
                    remote.encode(
                        TextureFormat::Bc4,
                        CookPreset::Balanced,
                        &[u32::MAX; 16],
                        &Cancellation::default()
                    ),
                    Err(Error::EncoderUnavailable(_))
                ));
            });
        }
    });
    assert_eq!(counts.calls.load(Ordering::Acquire), 32);
    assert_eq!(counts.peak.load(Ordering::Acquire), 2);
    drop(lease);
    done.recv_timeout(Duration::from_secs(2))
        .expect("service exits");
    Ok(())
}

#[test]
fn authentication_revision_and_bounds_are_checked() -> Result<()> {
    let (descriptor, done, _) = fixture()?;
    let lease = descriptor.endpoint.lease()?;
    let mut wrong = descriptor.clone();
    wrong.endpoint.token[0] ^= 1;
    assert!(Remote::connect(wrong).is_err());
    let mut wrong = descriptor.clone();
    wrong.identity.push('x');
    assert!(Remote::connect(wrong).is_err());
    assert!(
        wire::read_json::<String>(&mut std::io::Cursor::new((65537u32).to_le_bytes())).is_err()
    );
    assert!(wire::validate_count(TextureFormat::Bc7, wire::MAX_BLOCKS * 16 + 16).is_err());
    assert!(wire::validate_count(TextureFormat::Bc7, 15).is_err());
    assert!(wire::format(0).is_err());
    assert!(wire::preset(3).is_err());
    let remote = Remote::connect(descriptor)?;
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(matches!(
        remote.encode(TextureFormat::Bc7, CookPreset::Balanced, &[0; 16], &cancel),
        Err(Error::Cancelled)
    ));
    drop(remote);
    drop(lease);
    done.recv_timeout(Duration::from_secs(2))
        .expect("service exits");
    Ok(())
}
