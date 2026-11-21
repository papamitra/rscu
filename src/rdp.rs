use crate::overview::{Frame, Region, Target};
use ironrdp_server::{
    BitmapUpdate, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent, PixelFormat, RdpServer,
    RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler,
    tokio_rustls::{TlsAcceptor, rustls},
};
use std::{
    net::{Ipv4Addr, TcpListener},
    num::{NonZeroU16, NonZeroUsize},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, watch};

pub const WIDTH: usize = 1280;
pub const HEIGHT: usize = 720;

pub struct Preview {
    frames: watch::Sender<Option<Update>>,
    view: Arc<Mutex<View>>,
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
        let display_frames = frames.clone();
        let view = Arc::new(Mutex::new(View::default()));
        let connection_view = Arc::clone(&view);
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
                        let view = Arc::clone(&connection_view);
                        *view.lock().unwrap() = View::default();
                        let mut server = RdpServer::builder()
                            .with_addr(address)
                            .with_tls(tls.clone())
                            .with_input_handler(ViewInput {
                                view: Arc::clone(&view),
                                position: None,
                                pressed: None,
                            })
                            .with_display_handler(PreviewDisplay {
                                frames: display_frames.clone(),
                                view,
                            })
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
            view,
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

    pub fn selected(&self) -> Option<u64> {
        self.view.lock().unwrap().selected
    }

    pub fn publish(&self, requested: Option<u64>, shown: Option<u64>, frame: Frame) {
        let mut view = self.view.lock().unwrap();
        if view.selected != requested {
            return;
        }
        if shown != requested {
            view.select(shown);
        }
        let revision = view.revision;
        drop(view);
        self.frames.send_if_modified(|last| {
            if last.as_ref().is_some_and(|update| {
                update.revision == revision
                    && update.regions == frame.regions
                    && update.bitmap.data.as_ref() == frame.pixels
            }) {
                return false;
            }
            *last = Some(Update {
                revision,
                regions: frame.regions,
                bitmap: BitmapUpdate {
                    x: 0,
                    y: 0,
                    width: NonZeroU16::new(WIDTH as u16).unwrap(),
                    height: NonZeroU16::new(HEIGHT as u16).unwrap(),
                    format: PixelFormat::RgbX32,
                    stride: NonZeroUsize::new(WIDTH * 4).unwrap(),
                    data: frame.pixels.into(),
                },
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

#[derive(Default)]
struct View {
    selected: Option<u64>,
    revision: u64,
    layout_revision: u64,
    regions: Vec<Region>,
}

impl View {
    fn select(&mut self, selected: Option<u64>) {
        if self.selected != selected {
            self.selected = selected;
            self.revision += 1;
            self.present(&[]);
        }
    }

    fn present(&mut self, regions: &[Region]) {
        if self.regions != regions {
            self.layout_revision += 1;
            self.regions = regions.to_vec();
        }
    }
}

struct ViewInput {
    view: Arc<Mutex<View>>,
    position: Option<(u16, u16)>,
    pressed: Option<(Target, u64)>,
}

impl RdpServerInputHandler for ViewInput {
    fn keyboard(&mut self, event: KeyboardEvent) {
        if matches!(
            event,
            KeyboardEvent::Pressed {
                code: 1,
                extended: false
            }
        ) {
            self.pressed = None;
            self.view.lock().unwrap().select(None);
        }
    }

    fn mouse(&mut self, event: MouseEvent) {
        let down = match event {
            MouseEvent::Move { x, y } => {
                self.position = Some((x, y));
                return;
            }
            MouseEvent::LeftPressed => true,
            MouseEvent::LeftReleased => false,
            _ => return,
        };
        let mut view = self.view.lock().unwrap();
        let hit = self.position.and_then(|(x, y)| {
            view.regions
                .iter()
                .find(|region| region.contains(x, y))
                .map(|region| (region.target, view.layout_revision))
        });
        if down {
            self.pressed = hit;
        } else if let Some((target, _)) = self.pressed.take().filter(|press| Some(*press) == hit) {
            view.select(match target {
                Target::Window(id) => Some(id),
                Target::Back => None,
            });
        }
    }
}

struct Update {
    bitmap: BitmapUpdate,
    revision: u64,
    regions: Vec<Region>,
}

struct PreviewDisplay {
    frames: watch::Sender<Option<Update>>,
    view: Arc<Mutex<View>>,
}

#[async_trait::async_trait]
impl RdpServerDisplay for PreviewDisplay {
    async fn size(&mut self) -> DesktopSize {
        DesktopSize {
            width: WIDTH as u16,
            height: HEIGHT as u16,
        }
    }

    async fn updates(&mut self) -> anyhow::Result<Box<dyn RdpServerDisplayUpdates>> {
        self.frames.send_replace(None);
        Ok(Box::new(PreviewUpdates {
            frames: self.frames.subscribe(),
            view: Arc::clone(&self.view),
        }))
    }
}

struct PreviewUpdates {
    frames: watch::Receiver<Option<Update>>,
    view: Arc<Mutex<View>>,
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for PreviewUpdates {
    async fn next_update(&mut self) -> anyhow::Result<Option<DisplayUpdate>> {
        while self.frames.changed().await.is_ok() {
            if let Some(frame) = self.frames.borrow_and_update().as_ref() {
                let mut view = self.view.lock().unwrap();
                if view.revision == frame.revision {
                    view.present(&frame.regions);
                    return Ok(Some(DisplayUpdate::Bitmap(frame.bitmap.clone())));
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview::Overview;
    use smithay::utils::Rectangle;

    fn layout(ids: &[u64]) -> Frame {
        let mut overview = Overview::new(ids.len(), None);
        for (index, &id) in ids.iter().enumerate() {
            overview.tile(
                index,
                Target::Window(id),
                Rectangle::from_size((1, 1).into()),
                &[0; 3],
                false,
                None,
            );
        }
        overview.finish()
    }

    #[test]
    fn click_requires_same_target_and_layout() {
        let view = Arc::new(Mutex::new(View::default()));
        view.lock().unwrap().present(&layout(&[10, 20]).regions);
        let mut input = ViewInput {
            view: Arc::clone(&view),
            position: None,
            pressed: None,
        };
        input.mouse(MouseEvent::Move { x: 200, y: 300 });
        input.mouse(MouseEvent::LeftPressed);
        input.mouse(MouseEvent::Move { x: 1000, y: 300 });
        input.mouse(MouseEvent::LeftReleased);
        assert_eq!(view.lock().unwrap().selected, None);

        input.mouse(MouseEvent::LeftPressed);
        view.lock().unwrap().present(&layout(&[10, 30]).regions);
        view.lock().unwrap().present(&layout(&[10, 20]).regions);
        input.mouse(MouseEvent::LeftReleased);
        assert_eq!(view.lock().unwrap().selected, None);

        input.mouse(MouseEvent::LeftPressed);
        input.mouse(MouseEvent::LeftReleased);
        assert_eq!(view.lock().unwrap().selected, Some(20));
        input.keyboard(KeyboardEvent::Pressed {
            code: 1,
            extended: false,
        });
        assert_eq!(view.lock().unwrap().selected, None);
    }
}
