use mdns_sd::{AddrType, ServiceDaemon, ServiceInfo};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast::Receiver, watch};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::utils::{gen_mdns_endpoint_info, gen_mdns_name, DeviceType};

const INNER_NAME: &str = "MDnsServer";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Visibility {
    Visible = 0,
    Invisible = 1,
}

#[allow(dead_code)]
impl Visibility {
    pub fn from_raw_value(value: u8) -> Self {
        match value {
            0 => Visibility::Visible,
            _ => Visibility::Invisible,
        }
    }
}

pub struct MDnsServer {
    daemon: ServiceDaemon,
    service_info: ServiceInfo,
    ble_receiver: Receiver<()>,
    visibility_receiver: watch::Receiver<Visibility>,
}

impl MDnsServer {
    pub fn new(
        endpoint_id: [u8; 4],
        service_port: u16,
        ble_receiver: Receiver<()>,
        visibility_receiver: watch::Receiver<Visibility>,
    ) -> Result<Self, anyhow::Error> {
        let service_info = Self::build_service(endpoint_id, service_port, DeviceType::Laptop)?;

        Ok(Self {
            daemon: ServiceDaemon::new()?,
            service_info,
            ble_receiver,
            visibility_receiver,
        })
    }

    pub async fn run(&mut self, ctk: CancellationToken) -> Result<(), anyhow::Error> {
        info!("{INNER_NAME}: service starting");
        let monitor = self.daemon.monitor()?;
        let ble_receiver = &mut self.ble_receiver;
        let mut visibility = *self.visibility_receiver.borrow();

        loop {
            tokio::select! {
                _ = ctk.cancelled() => {
                    info!("{INNER_NAME}: tracker cancelled, breaking");
                    break;
                }
                r = monitor.recv_async() => {
                    match r {
                        Ok(_) => continue,
                        Err(err) => return Err(err.into()),
                    }
                },
                _ = self.visibility_receiver.changed() => {
                    visibility = *self.visibility_receiver.borrow_and_update();

                    debug!("{INNER_NAME}: visibility changed: {visibility:?}");
                    if visibility == Visibility::Visible {
                        self.daemon.register(self.service_info.clone())?;
                    } else if visibility == Visibility::Invisible {
                        let receiver = self.daemon.unregister(self.service_info.get_fullname())?;
                        let _ = receiver.recv();
                    }
                }
                _ = ble_receiver.recv() => {
                    if visibility == Visibility::Invisible {
                        continue;
                    }

                    debug!("{INNER_NAME}: ble_receiver: got event");
                    if visibility == Visibility::Visible {
                        // Android can sometime not see the mDNS service if the service
                        // was running BEFORE Android started the Discovery phase for QuickShare.
                        // So resend a broadcast if there's a android device sending.
                        self.daemon.register_resend(self.service_info.get_fullname())?;
                    } else {
                        self.daemon.register(self.service_info.clone())?;
                    }
                }
            }
        }

        // Unregister the mDNS service - we're shutting down
        let receiver = self.daemon.unregister(self.service_info.get_fullname())?;
        if let Ok(event) = receiver.recv() {
            info!("MDnsServer: service unregistered: {:?}", &event);
        }

        Ok(())
    }

    fn build_service(
        endpoint_id: [u8; 4],
        service_port: u16,
        device_type: DeviceType,
    ) -> Result<ServiceInfo, anyhow::Error> {
        let name = gen_mdns_name(endpoint_id);
        let hostname = sys_metrics::host::get_hostname()?;
        info!("Broadcasting with: {hostname}");
        let endpoint_info = gen_mdns_endpoint_info(device_type as u8, &hostname);

        let properties = [("n", endpoint_info)];
        let si = ServiceInfo::new(
            "_FC9F5ED42C8A._tcp.local.",
            &name,
            &hostname,
            "",
            service_port,
            &properties[..],
        )?
        .enable_addr_auto(AddrType::V4);

        Ok(si)
    }
}
