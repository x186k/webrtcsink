use crate::webrtcsink::WebRTCSink;
use anyhow::{anyhow, Error};
use async_std::task;
use futures::channel::mpsc;
use futures::prelude::*;
use gst::glib::prelude::*;
use gst::glib::{self, WeakRef};
use gst::subclass::prelude::*;
use once_cell::sync::Lazy;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "webrtcsink-signaller",
        gst::DebugColorFlags::empty(),
        Some("WebRTC sink signaller"),
    )
});

#[derive(Default)]
struct State {
    /// Sender for the websocket messages
    websocket_sender: Option<mpsc::Sender<WhipMessage>>,
    send_task_handle: Option<task::JoinHandle<Result<(), Error>>>,
    receive_task_handle: Option<task::JoinHandle<()>>,
}

#[derive(Clone)]
struct Settings {
    address: Option<String>,
    cafile: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            address: Some("ws://127.0.0.1:8443".to_string()),
            cafile: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WhipMessage {
    Ice { id: String, candidate: String, candix: u32 },
    Sdp { id: String, sdp: String },
    ConsumerRemoved { id: String },
    GatherTimeout { id: String },
    //List,
}

#[derive(Default)]
pub struct Signaller {
    state: Mutex<State>,
    settings: Mutex<Settings>,
}

impl Signaller {
    async fn connect(&self, element: &WebRTCSink) -> Result<(), Error> {
        let _settings = self.settings.lock().unwrap().clone();

        gst::info!(CAT, obj: element, "connect called");

        // removed ws setup

        let mut xsdp = "".to_string();

        // let a = future::ready(1).delay(Duration::from_millis(2000));
        // dbg!(a.await);

        // 1000 is completely arbitrary, we simply don't want infinite piling
        // up of messages as with unbounded
        let (whip_sender, mut whip_receiver) = mpsc::channel::<WhipMessage>(1000);

        let w1 = whip_sender.clone();

        let element_clone = element.downgrade();
        let send_task_handle = task::spawn(async move {
            while let Some(msg) = whip_receiver.next().await {
                if let Some(element) = element_clone.upgrade() {
                    gst::trace!(CAT, obj: &element, "Sending websocket message {:?}", msg);
                }
                //println!("got whip msg: {:?}", msg);
                // removed wssend

                // testing
                match msg {
                    WhipMessage::Ice {
                        id,
                        candidate,
                        candix: _,
                    } => {
                        println!("..ice");
                        if candidate.contains("candidate:18") {
                            println!("..18.");
                            //println!("{}", xsdp);

                            if let Err(err) = do_whip(element_clone.clone(), id, xsdp.clone()) {
                                if let Some(element) = element_clone.upgrade() {
                                    element.handle_signalling_error(err.into());
                                }
                            }
                        }

                        writeln!(xsdp, "a={}", candidate).unwrap();
                    }
                    WhipMessage::Sdp { id: _, sdp } => {
                        println!("..sdp");
                        let mut w2 = w1.clone();

                        let element_cl1 = element_clone.clone();
                        task::spawn(async move {
                            task::sleep(std::time::Duration::from_millis(500)).await;
                            if let Err(err) = w2.send(WhipMessage::GatherTimeout { id: "".to_string() }).await {
                                if let Some(element) = element_cl1.upgrade() {
                                    element.handle_signalling_error(err.into());
                                }
                            }
                        });

                        write!(xsdp, "{}", sdp).unwrap();
                    }
                    WhipMessage::ConsumerRemoved { id: _ } => fun_name(),
                    WhipMessage::GatherTimeout { id: _ } => println!("..GatherTimeout"),
                }
            }

            if let Some(element) = element_clone.upgrade() {
                gst::info!(CAT, obj: &element, "Done sending");
            }

            // removed ws shutdown

            Ok::<(), Error>(())
        });

        // whip_sender
        //     .send(p::IncomingMessage::Register(p::RegisterMessage::Producer {
        //         display_name: element.property("display-name"),
        //     }))
        //     .await?;

        let element_clone = element.downgrade();
        let receive_task_handle = task::spawn(async move {
            //removed ws receiver

            if let Some(element) = element_clone.upgrade() {
                gst::info!(CAT, obj: &element, "Stopped websocket receiving");
            }
        });

        let mut state = self.state.lock().unwrap();
        state.websocket_sender = Some(whip_sender);
        state.send_task_handle = Some(send_task_handle);
        state.receive_task_handle = Some(receive_task_handle);

        // start everything rolling
        element.add_consumer("xid")?;

        Ok(())
    }

    pub fn start(&self, element: &WebRTCSink) {
        let this = self.instance();
        let element_clone = element.clone();
        task::spawn(async move {
            let this = Self::from_instance(&this);
            if let Err(err) = this.connect(&element_clone).await {
                element_clone.handle_signalling_error(err.into());
            }
        });
    }

    pub fn handle_sdp(&self, element: &WebRTCSink, peer_id: &str, sdp: &gst_webrtc::WebRTCSessionDescription) {
        let state = self.state.lock().unwrap();

        // let msg = p::IncomingMessage::Peer(p::PeerMessage {
        //     peer_id: peer_id.to_string(),
        //     peer_message: p::PeerMessageInner::Sdp(p::SdpMessage::Offer {
        //         sdp: sdp.sdp().as_text().unwrap(),
        //     }),
        // });

        let msg = WhipMessage::Sdp {
            id: peer_id.to_string(),
            sdp: sdp.sdp().as_text().unwrap(),
        };

        if let Some(mut sender) = state.websocket_sender.clone() {
            let element = element.downgrade();
            task::spawn(async move {
                if let Err(err) = sender.send(msg).await {
                    if let Some(element) = element.upgrade() {
                        element.handle_signalling_error(anyhow!("Error: {}", err).into());
                    }
                }
            });
        }
    }

    pub fn handle_ice(
        &self,
        element: &WebRTCSink,
        peer_id: &str,
        candidate: &str,
        sdp_m_line_index: Option<u32>,
        _sdp_mid: Option<String>,
    ) {
        let state = self.state.lock().unwrap();

        // let msg = p::IncomingMessage::Peer(p::PeerMessage {
        //     peer_id: peer_id.to_string(),
        //     peer_message: p::PeerMessageInner::Ice {
        //         candidate: candidate.to_string(),
        //         sdp_m_line_index: sdp_m_line_index.unwrap(),
        //     },
        // });

        let msg = WhipMessage::Ice {
            id: peer_id.to_string(),
            candidate: candidate.to_string(),
            candix: sdp_m_line_index.unwrap(),
        };

        if let Some(mut sender) = state.websocket_sender.clone() {
            let element = element.downgrade();
            task::spawn(async move {
                if let Err(err) = sender.send(msg).await {
                    if let Some(element) = element.upgrade() {
                        element.handle_signalling_error(anyhow!("Error: {}", err).into());
                    }
                }
            });
        }
    }

    pub fn stop(&self, element: &WebRTCSink) {
        gst::info!(CAT, obj: element, "Stopping now");

        let mut state = self.state.lock().unwrap();
        let send_task_handle = state.send_task_handle.take();
        let receive_task_handle = state.receive_task_handle.take();
        if let Some(mut sender) = state.websocket_sender.take() {
            task::block_on(async move {
                sender.close_channel();

                if let Some(handle) = send_task_handle {
                    if let Err(err) = handle.await {
                        gst::warning!(CAT, obj: element, "Error while joining send task: {}", err);
                    }
                }

                if let Some(handle) = receive_task_handle {
                    handle.await;
                }
            });
        }
    }

    pub fn consumer_removed(&self, element: &WebRTCSink, peer_id: &str) {
        gst::debug!(CAT, obj: element, "Signalling consumer {} removed", peer_id);

        let state = self.state.lock().unwrap();
        let peer_id = peer_id.to_string();
        let element = element.downgrade();
        if let Some(mut sender) = state.websocket_sender.clone() {
            task::spawn(async move {
                if let Err(err) = sender.send(WhipMessage::ConsumerRemoved { id: peer_id.to_string() }).await {
                    if let Some(element) = element.upgrade() {
                        element.handle_signalling_error(anyhow!("Error: {}", err).into());
                    }
                }
            });
        }
    }
}

fn fun_name() {
    println!("..ConsumerRemoved")
}

fn do_whip(element_clone: WeakRef<WebRTCSink>, peer_id: String, mut xsdp: String) -> Result<(), Error> {
    writeln!(xsdp, "a=end-of-candidates").unwrap();

    // println!("full sdp {}", xsdp);

    println!("pre post: {}", &xsdp);

    let rq = reqwest::blocking::Client::new();
    let r = rq
        // .post("http://localhost:3000/foo")
        .post("http://192.168.86.3:7080/whip/endpoint/foo")
        .header("Content-type", "application/sdp")
        .body(xsdp)
        .send()?;

    println!("post, post");

    if let Some(loc) = r.headers().get("Location") {
        println!("location found: {:?}", loc);
    }
    let answer_sdp = r.bytes()?.to_vec();

    // println!("answer_sdp {}", String::from_utf8(answer_sdp.clone())?);

    println!("######### call handle_sdp");

    // drop(state);
    if let Some(element) = element_clone.upgrade() {
        gst::trace!(CAT, obj: &element, "Giving SDP to sink");

        element.handle_sdp(
            &peer_id,
            &gst_webrtc::WebRTCSessionDescription::new(
                gst_webrtc::WebRTCSDPType::Answer,
                gst_sdp::SDPMessage::parse_buffer(&answer_sdp.clone()).unwrap(),
            ),
        )?;
    }

    Ok(())
}

#[glib::object_subclass]
impl ObjectSubclass for Signaller {
    const NAME: &'static str = "RsWebRTCSinkSignaller";
    type Type = super::Signaller;
    type ParentType = glib::Object;
}

impl ObjectImpl for Signaller {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| {
            vec![
                glib::ParamSpecString::new(
                    "address",
                    "Address",
                    "Address of the signalling server",
                    Some("ws://127.0.0.1:8443"),
                    glib::ParamFlags::READWRITE,
                ),
                glib::ParamSpecString::new(
                    "cafile",
                    "CA file",
                    "Path to a Certificate file to add to the set of roots the TLS connector will trust",
                    None,
                    glib::ParamFlags::READWRITE,
                ),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _obj: &Self::Type, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "address" => {
                let address: Option<_> = value.get().expect("type checked upstream");

                if let Some(address) = address {
                    gst::info!(CAT, "Signaller address set to {}", address);

                    let mut settings = self.settings.lock().unwrap();
                    settings.address = Some(address);
                } else {
                    gst::error!(CAT, "address can't be None");
                }
            }
            "cafile" => {
                let value: String = value.get().unwrap();
                let mut settings = self.settings.lock().unwrap();
                settings.cafile = Some(value.into());
            }
            _ => unimplemented!(),
        }
    }

    fn property(&self, _obj: &Self::Type, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "address" => self.settings.lock().unwrap().address.to_value(),
            "cafile" => {
                let settings = self.settings.lock().unwrap();
                let cafile = settings.cafile.as_ref();
                cafile.and_then(|file| file.to_str()).to_value()
            }
            _ => unimplemented!(),
        }
    }
}
