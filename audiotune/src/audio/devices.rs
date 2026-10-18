//! Enumerating hosts and I/O devices through cpal (WASAPI on Windows, ALSA/JACK on Linux).

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{HostId, SampleFormat};

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub max_channels: u16,
    pub sample_rates: Vec<u32>,
    pub formats: Vec<SampleFormat>,
    pub default_sample_rate: u32,
    pub default_channels: u16,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceList {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

pub fn host_label(id: HostId) -> String {
    id.name().to_string()
}

pub fn available_hosts() -> Vec<HostId> {
    cpal::available_hosts()
}

/// Sample rates worth offering; only the ones the device really supports are kept.
const COMMON_RATES: [u32; 8] = [44100, 48000, 88200, 96000, 176400, 192000, 32000, 22050];

fn collect(device: &cpal::Device, input: bool, is_default: bool) -> Option<DeviceInfo> {
    let name = device.name().unwrap_or_else(|_| "?".into());
    let configs: Vec<cpal::SupportedStreamConfigRange> = if input {
        device.supported_input_configs().ok()?.collect()
    } else {
        device.supported_output_configs().ok()?.collect()
    };
    if configs.is_empty() {
        return None;
    }
    let mut rates: Vec<u32> = COMMON_RATES
        .iter()
        .copied()
        .filter(|r| {
            configs
                .iter()
                .any(|c| c.min_sample_rate().0 <= *r && *r <= c.max_sample_rate().0)
        })
        .collect();
    rates.sort_unstable();
    rates.dedup();

    let mut formats: Vec<SampleFormat> = configs.iter().map(|c| c.sample_format()).collect();
    formats.sort_by_key(|f| format!("{f:?}"));
    formats.dedup();

    let default = if input {
        device.default_input_config().ok()
    } else {
        device.default_output_config().ok()
    };
    let (default_sample_rate, default_channels) = default
        .map(|c| (c.sample_rate().0, c.channels()))
        .unwrap_or((48000, if input { 1 } else { 2 }));

    if rates.is_empty() {
        rates.push(default_sample_rate);
    }

    Some(DeviceInfo {
        name,
        is_default,
        max_channels: configs.iter().map(|c| c.channels()).max().unwrap_or(2),
        sample_rates: rates,
        formats,
        default_sample_rate,
        default_channels,
    })
}

pub fn enumerate(host_id: HostId) -> DeviceList {
    let Ok(host) = cpal::host_from_id(host_id) else {
        return DeviceList::default();
    };
    let default_in = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    let default_out = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let mut inputs = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let is_def = d.name().map(|n| n == default_in).unwrap_or(false);
            if let Some(i) = collect(&d, true, is_def) {
                inputs.push(i);
            }
        }
    }
    let mut outputs = Vec::new();
    if let Ok(devs) = host.output_devices() {
        for d in devs {
            let is_def = d.name().map(|n| n == default_out).unwrap_or(false);
            if let Some(i) = collect(&d, false, is_def) {
                outputs.push(i);
            }
        }
    }
    // Put the default device first so it is easy to pick.
    inputs.sort_by_key(|d| !d.is_default);
    outputs.sort_by_key(|d| !d.is_default);
    DeviceList { inputs, outputs }
}

pub fn find_device(host_id: HostId, name: &str, input: bool) -> Option<cpal::Device> {
    let host = cpal::host_from_id(host_id).ok()?;
    let devs: Vec<cpal::Device> = if input {
        host.input_devices().ok()?.collect()
    } else {
        host.output_devices().ok()?.collect()
    };
    devs.into_iter()
        .find(|d| d.name().map(|n| n == name).unwrap_or(false))
}

/// Choose the best sample format for a device at the chosen rate.
pub fn pick_format(
    device: &cpal::Device,
    input: bool,
    channels: u16,
    sample_rate: u32,
) -> Option<SampleFormat> {
    let configs: Vec<cpal::SupportedStreamConfigRange> = if input {
        device.supported_input_configs().ok()?.collect()
    } else {
        device.supported_output_configs().ok()?.collect()
    };
    let matching: Vec<&cpal::SupportedStreamConfigRange> = configs
        .iter()
        .filter(|c| {
            c.channels() >= channels
                && c.min_sample_rate().0 <= sample_rate
                && sample_rate <= c.max_sample_rate().0
        })
        .collect();
    for want in [SampleFormat::F32, SampleFormat::I16, SampleFormat::U16] {
        if matching.iter().any(|c| c.sample_format() == want) {
            return Some(want);
        }
    }
    matching.first().map(|c| c.sample_format())
}
