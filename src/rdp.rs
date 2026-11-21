use ironrdp_server::{
    BitmapUpdate, DesktopSize, DisplayUpdate, PixelFormat, RdpServer, RdpServerDisplay,
    RdpServerDisplayUpdates,
    tokio_rustls::{TlsAcceptor, rustls},
};
use std::{
    net::{Ipv4Addr, TcpListener},
    num::{NonZeroU16, NonZeroUsize},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch};

pub const WIDTH: usize = 1280;
pub const HEIGHT: usize = 720;

pub struct Preview {
    frames: watch::Sender<Option<BitmapUpdate>>,
    shutdown: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    next_frame: Instant,
}

impl Preview {
    pub fn start(port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let certificate =
            rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
        let key =
            rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der());
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(vec![certificate.cert.der().clone()], key.into())?;
        let tls = TlsAcceptor::from(Arc::new(config));
        let (frames, _) = watch::channel(None);
        let display = PreviewDisplay(frames.clone());
        let (shutdown, mut stopping) = oneshot::channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let worker = thread::Builder::new()
            .name("rscu-rdp".into())
            .spawn(move || {
                let result = runtime.block_on(async move {
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    loop {
                        let (socket, _) = tokio::select! {
                            _ = &mut stopping => break,
                            result = listener.accept() => result?,
                        };
                        socket.set_nodelay(true)?;
                        let mut server = RdpServer::builder()
                            .with_addr(address)
                            .with_tls(tls.clone())
                            .with_no_input()
                            .with_display_handler(PreviewDisplay(display.0.clone()))
                            .build();
                        tokio::select! {
                            _ = &mut stopping => break,
                            result = server.run_connection(socket) => {
                                if let Err(error) = result {
                                    eprintln!("RDP connection: {error:#}");
                                }
                            }
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                });
                if let Err(error) = result {
                    eprintln!("RDP server: {error:#}");
                }
            })?;
        eprintln!("RDP viewer: {address}");
        Ok(Self {
            frames,
            shutdown: Some(shutdown),
            worker: Some(worker),
            next_frame: Instant::now(),
        })
    }

    pub fn frame_due(&mut self) -> bool {
        let now = Instant::now();
        if self.frames.receiver_count() == 0 || now < self.next_frame {
            return false;
        }
        self.next_frame = now + Duration::from_millis(67);
        true
    }

    pub fn publish(&self, pixels: Vec<u8>) {
        self.frames.send_if_modified(|last| {
            if last
                .as_ref()
                .is_some_and(|frame| frame.data.as_ref() == pixels)
            {
                return false;
            }
            *last = Some(BitmapUpdate {
                x: 0,
                y: 0,
                width: NonZeroU16::new(WIDTH as u16).unwrap(),
                height: NonZeroU16::new(HEIGHT as u16).unwrap(),
                format: PixelFormat::RgbX32,
                stride: NonZeroUsize::new(WIDTH * 4).unwrap(),
                data: pixels.into(),
            });
            true
        });
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct PreviewDisplay(watch::Sender<Option<BitmapUpdate>>);

#[async_trait::async_trait]
impl RdpServerDisplay for PreviewDisplay {
    async fn size(&mut self) -> DesktopSize {
        DesktopSize {
            width: WIDTH as u16,
            height: HEIGHT as u16,
        }
    }

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        self.0.send_replace(None);
        Ok(Box::new(PreviewUpdates(self.0.subscribe())))
    }
}

struct PreviewUpdates(watch::Receiver<Option<BitmapUpdate>>);

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for PreviewUpdates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        while self.0.changed().await.is_ok() {
            if let Some(frame) = self.0.borrow_and_update().as_ref() {
                return Ok(Some(DisplayUpdate::Bitmap(frame.clone())));
            }
        }
        Ok(None)
    }
}
