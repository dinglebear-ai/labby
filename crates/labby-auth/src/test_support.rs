use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SharedWriter(Arc::clone(&self.0))
    }
}

impl SharedBuf {
    fn clear(&self) {
        self.0.lock().expect("capture buffer lock").clear();
    }
}

pub struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("capture buffer lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn captured_logs(buf: &SharedBuf) -> String {
    String::from_utf8(buf.0.lock().expect("capture buffer lock").clone())
        .expect("captured logs are utf-8")
}

pub fn global_tracing_buffer() -> &'static SharedBuf {
    static BUFFER: OnceLock<SharedBuf> = OnceLock::new();
    static SUBSCRIBER: OnceLock<()> = OnceLock::new();

    let buffer = BUFFER.get_or_init(SharedBuf::default);
    SUBSCRIBER.get_or_init(|| {
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("labby_auth=debug"))
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(buffer.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        tracing::subscriber::set_global_default(subscriber)
            .expect("install labby-auth test tracing subscriber");
    });
    buffer.clear();
    buffer
}

pub static TRACING_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
