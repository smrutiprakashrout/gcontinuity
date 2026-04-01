use anyhow::Result;
use std::time::Duration;
use zeroconf::prelude::*;
use zeroconf::{MdnsService as ZcMdnsService, ServiceRegistration, ServiceType, TxtRecord};

pub struct MdnsService {
    pub device_name: String,
    pub device_id: String,
    pub port: u16,
}

impl MdnsService {
    pub fn new(device_name: String, device_id: String) -> Self {
        Self {
            device_name,
            device_id,
            port: 52000,
        }
    }

    pub fn advertise(&self) -> Result<()> {
        let mut service = ZcMdnsService::new(
            ServiceType::new("_gcontinuity", "_tcp")?,
            self.port,
        );
        service.set_name(&self.device_name);

        let mut txt_record = TxtRecord::new();
        txt_record.insert("id", &self.device_id)?;
        txt_record.insert("name", &self.device_name)?;
        txt_record.insert("version", "1")?;
        service.set_txt_record(txt_record);

        service.set_registered_callback(Box::new(|result: zeroconf::Result<ServiceRegistration>, _| {
            match result {
                Ok(context) => tracing::info!("mDNS registered: {}", context.name()),
                Err(e) => tracing::error!("mDNS registration failed: {}", e),
            }
        }));

        let event_loop = service.register()?;

        tracing::info!("Starting mDNS event loop for _gcontinuity._tcp on port {}", 52000);
        loop {
            event_loop.poll(Duration::from_secs(1))?;
        }
    }

    pub async fn run_in_background(self) -> tokio::task::JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            if let Err(e) = self.advertise() {
                tracing::error!("mDNS advertisement failed: {}", e);
            }
        })
    }
}
