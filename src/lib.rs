#[macro_use]
extern crate log;
#[macro_use]
extern crate wooting_analog_plugin_dev;
extern crate hidapi;
#[macro_use]
extern crate objekt;
extern crate timer;
extern crate chrono;


use hidapi::{HidApi, HidDevice, HidDeviceInfo};
use log::{error, info};
use std::collections::HashMap;
use std::os::raw::{c_float, c_int, c_ushort};
use std::str;
use wooting_analog_plugin_dev::wooting_analog_common::*;
use wooting_analog_plugin_dev::*;
use timer::{Guard, Timer};
use std::sync::{Mutex, Arc};
use std::borrow::Borrow;

extern crate env_logger;

const ANALOG_BUFFER_SIZE: usize = 48;

/// Struct holding the information we need to find the device and the analog interface
struct DeviceHardwareID {
    vid: u16,
    pid: u16,
    usage_page: u16,
    interface_n: i32,
}

/// Trait which defines how the Plugin can communicate with a particular device
trait DeviceImplementation: objekt::Clone + Send {
    /// Gives the device hardware ID that can be used to obtain the analog interface for this device
    fn device_hardware_id(&self) -> DeviceHardwareID;

    /// Used to determine if the given `device` matches the hardware id given by `device_hardware_id`
    fn matches(&self, device: &HidDeviceInfo) -> bool {
        let hid = self.device_hardware_id();
        //Check if the pid & hid match
        device.product_id.eq(&hid.pid)
            && device.vendor_id.eq(&hid.vid)
            && if device.usage_page != 0 && hid.usage_page != 0
            //check if the usage_page is valid to check
            {
                //if it is, check if they are the same
                device.usage_page.eq(&hid.usage_page)
            } else {
                //otherwise, check if the defined interface number is correct
                (hid.interface_n.eq(&device.interface_number))
            }
    }

    /// Convert the given raw `value` into the appropriate float value. The given value should be 0.0f-1.0f
    fn analog_value_to_float(&self, value: u8) -> f32 {
        (f32::from(value) / 255_f32).min(1.0)
    }

    /// Get the current set of pressed keys and their analog values from the given `device`. Using `buffer` to read into
    ///
    /// # Notes
    /// `buffer` is used to prevent continually allocating & deallocating memory and so that HID `read_timeout` can be used with
    /// `0` time to get data async without having cases where you end up with an empty buffer because the read wasn't fast enough
    ///
    /// `max_length` is not the max length of the report, it is the max number of key + analog value pairs to read
    fn get_analog_buffer(
        &self,
        buffer: &mut [u8],
        device: &HidDevice,
        max_length: usize,
    ) -> SDKResult<HashMap<c_ushort, c_float>> {
        let res = device.read_timeout(buffer, 0);
        if let Err(e) = res {
            error!("Failed to read buffer: {}", e);

            return WootingAnalogResult::DeviceDisconnected.into();
        }
        //println!("{:?}", buffer);
        Ok(buffer
            .chunks_exact(3) //Split it into groups of 3 as the analog report is in the format of 2 byte code + 1 byte analog value
            .take(max_length) //Only take up to the max length of results. Doing this
            .filter(|&s| s[2] != 0) //Get rid of entries where the analog value is 0
            .map(|s| {
                (
                    ((u16::from(s[0])) << 8) | u16::from(s[1]), // Convert the first 2 bytes into the u16 code
                    self.analog_value_to_float(s[2]), //Convert the remaining byte into the float analog value
                )
            })
            .collect())
        .into()
    }

    /// Get the unique device ID from the given `device_info`
    fn get_device_id(&self, device_info: &HidDeviceInfo) -> DeviceID {
        wooting_analog_plugin_dev::generate_device_id(
            device_info
                .serial_number
                .as_ref()
                .unwrap_or(&String::from("NO SERIAL")),
            device_info.vendor_id,
            device_info.product_id,
        )
    }
}

clone_trait_object!(DeviceImplementation);

#[derive(Debug, Clone)]
struct WootingOne();

impl DeviceImplementation for WootingOne {
    fn device_hardware_id(&self) -> DeviceHardwareID {
        DeviceHardwareID {
            vid: 0x03EB,
            pid: 0xFF01,

            #[cfg(linux)]
            usage_page: 0,
            #[cfg(not(linux))]
            usage_page: 0xFF54,

            interface_n: 6,
        }
    }

    fn analog_value_to_float(&self, value: u8) -> f32 {
        ((f32::from(value) * 1.2) / 255_f32).min(1.0)
    }
}

#[derive(Debug, Clone)]
struct WootingTwo();

impl DeviceImplementation for WootingTwo {
    fn device_hardware_id(&self) -> DeviceHardwareID {
        DeviceHardwareID {
            vid: 0x03EB,
            pid: 0xFF02,

            #[cfg(linux)]
            usage_page: 0,
            #[cfg(not(linux))]
            usage_page: 0xFF54,

            interface_n: 6,
        }
    }

    fn analog_value_to_float(&self, value: u8) -> f32 {
        ((f32::from(value) * 1.2) / 255_f32).min(1.0)
    }
}

/// A fully contained device which uses `device_impl` to interface with the `device`
struct Device {
    device: HidDevice,
    pub device_info: DeviceInfoPointer,
    device_impl: Box<dyn DeviceImplementation>,
    buffer: [u8; ANALOG_BUFFER_SIZE],
}
unsafe impl Send for Device {}

impl Device {
    fn new(
        device_info: &HidDeviceInfo,
        device: HidDevice,
        device_impl: Box<DeviceImplementation>,
    ) -> (DeviceID, Self) {
        let id_hash = device_impl.get_device_id(device_info);
        (
            id_hash,
            Device {
                device,
                device_info: DeviceInfo::new_with_id(
                    device_info.vendor_id,
                    device_info.product_id,
                    device_info.manufacturer_string.as_ref().unwrap(),
                    device_info.product_string.as_ref().unwrap(),
                    id_hash,
                )
                .to_ptr(),
                device_impl,
                buffer: [0; ANALOG_BUFFER_SIZE],
            },
        )
    }

    fn read_analog(&mut self, code: u16) -> SDKResult<c_float> {
        match self
            .device_impl
            .get_analog_buffer(&mut self.buffer, &self.device, ANALOG_BUFFER_SIZE)
            .into()
        {
            Ok(data) => (*data.get(&code).unwrap_or(&0.0)).into(),
            Err(e) => Err(e).into(),
        }
    }

    fn read_full_buffer(&mut self, max_length: usize) -> SDKResult<HashMap<c_ushort, c_float>> {
        self.device_impl
            .get_analog_buffer(&mut self.buffer, &self.device, max_length)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.device_info.clone().drop();
    }
}

fn call_cb(cb: &Option<extern "C" fn(DeviceEventType, DeviceInfoPointer)>, device: &Device, event_type: DeviceEventType) {
    if let Some(cb) = cb {
        cb(event_type, device.device_info.clone());
    }
}

fn handle_device_event(cb: &Option<extern "C" fn(DeviceEventType, DeviceInfoPointer)>, device: &Device, cb_type: DeviceEventType) {
    call_cb(cb, device, cb_type);
}

pub struct WootingPlugin {
    initialised: bool,
    device_event_cb: Arc<Mutex<Option<extern "C" fn(DeviceEventType, DeviceInfoPointer)>>>,
    devices: Arc<Mutex<HashMap<DeviceID, Device>>>,
    timer: Timer,
    worker_guard: Option<Guard>
}

const PLUGIN_NAME: &str = "Wooting Official Plugin";
impl WootingPlugin {
    fn new() -> Self {
        WootingPlugin {
            initialised: false,
            device_event_cb:  Arc::new(Mutex::new(None)),
            devices: Arc::new(Mutex::new(Default::default())),
            timer: timer::Timer::new(),
            worker_guard: None
        }
    }
    
    fn init_worker(&mut self) -> WootingAnalogResult {
        let init_device_closure = |hid: &HidApi, devices: &Arc<Mutex<HashMap<DeviceID, Device>>>, device_event_cb: &Arc<Mutex<Option<extern "C" fn(DeviceEventType, DeviceInfoPointer)>>>, device_impls: &Vec<Box<dyn DeviceImplementation>>| {
            for device_info in hid.devices() {
                for device_impl in device_impls.iter() {
                    if device_impl.matches(device_info)
                        && !devices.lock().unwrap()
                        .contains_key(&device_impl.get_device_id(device_info))
                    {
                        info!("Found device impl match: {:?}", device_info);
                        match device_info.open_device(&hid) {
                            Ok(dev) => {
                                let (id, device) =
                                    Device::new(device_info, dev, device_impl.clone());
                                devices.lock().unwrap().insert(id, device);
                                info!(
                                    "Found and opened the {:?} successfully!",
                                    device_info.product_string
                                );
                                handle_device_event(
                                    device_event_cb.lock().unwrap().borrow(),
                                    devices.lock().unwrap().get(&id).unwrap(),
                                    DeviceEventType::Connected,
                                );
                            }
                            Err(e) => {
                                error!("Error opening HID Device: {}", e);
                                //return WootingAnalogResult::Failure.into();
                            }
                        }
                    }
                }
            }
        };

        let device_impls: Vec<Box<dyn DeviceImplementation>> = vec![Box::new(WootingOne()), Box::new(WootingTwo())];
        let mut hid = match HidApi::new() {
            Ok(api) => {
                api
            }
            Err(e) => {
                error!("Error obtaining HIDAPI: {}", e);
                return WootingAnalogResult::Failure;
            }
        };

        //We wanna call it in this thread first so we can get hold of any connected devices now so we can return an accurate result for initialise
        init_device_closure(&hid, &self.devices, &self.device_event_cb, &device_impls);

        self.worker_guard = Some({
            let t_devices = Arc::clone(&self.devices);
            let t_device_event_cb = Arc::clone(&self.device_event_cb);
            self.timer.schedule_repeating(chrono::Duration::seconds(1), move || {
                hid.refresh_devices();
                init_device_closure(&hid, &t_devices, &t_device_event_cb, &device_impls);
            })
        });
        debug!("Started timer");
        return if self.devices.lock().unwrap().is_empty() { WootingAnalogResult::NoDevices } else { WootingAnalogResult::Ok };
    }

    /*fn init_device(&mut self) -> WootingAnalogResult {
        self.hid_api.as_mut().map(|api| api.refresh_devices());

        match &self.hid_api {
            Some(api) => {
                for device_info in api.devices() {
                    //debug!("{:#?}", device_info);
                    for device_impl in self.device_impls.iter() {
                        if device_impl.matches(device_info)
                            && !self
                                .devices
                                .contains_key(&device_impl.get_device_id(device_info))
                        {
                            match device_info.open_device(&api) {
                                Ok(dev) => {
                                    let (id, device) =
                                        Device::new(device_info, dev, device_impl.clone());
                                    self.devices.insert(id, device);
                                    info!(
                                        "Found and opened the {:?} successfully!",
                                        device_info.product_string
                                    );
                                    handle_device_event(
                                        self.devices.get(&id).unwrap(),
                                        DeviceEventType::Connected,
                                    );
                                }
                                Err(e) => {
                                    error!("Error opening HID Device: {}", e);
                                    //return WootingAnalogResult::Failure.into();
                                }
                            }
                        }
                    }
                }

                if self.devices.is_empty() {
                    return WootingAnalogResult::NoDevices;
                }

                debug!("Finished with devices");
            }
            None => {
                return WootingAnalogResult::UnInitialized;
            }
        }
        WootingAnalogResult::Ok
    }*/

    
}

impl Plugin for WootingPlugin {
    fn name(&mut self) -> SDKResult<&'static str> {
        Ok(PLUGIN_NAME).into()
    }

    fn initialise(&mut self) -> WootingAnalogResult {
        //return WootingAnalogResult::Failure;
        env_logger::try_init();
        /*match HidApi::new() {
            Ok(api) => {
                self.hid_api = Some(api);
            }
            Err(e) => {
                error!("Error: {}", e);
                return WootingAnalogResult::Failure;
            }
        }*/
        let ret = self.init_worker();
        /*let ret = self.init_device();
        self.initialised = ret.is_ok();
        if self.initialised {
            info!("{} initialised", PLUGIN_NAME);
        }
        ret*/
        self.initialised = ret.is_ok() || ret == WootingAnalogResult::NoDevices;
        ret
    }

    fn is_initialised(&mut self) -> bool {
        self.initialised
    }

    fn unload(&mut self) {
        info!("{} unloaded", PLUGIN_NAME);
        //TODO: drop devices

        /*if self.device_info.is_some() {
            let dev = self.device_info.take();
            dev.unwrap().drop();
        }*/
    }

    fn set_device_event_cb(
        &mut self,
        cb: extern "C" fn(DeviceEventType, DeviceInfoPointer),
    ) -> WootingAnalogResult {
        if !self.initialised {
            return WootingAnalogResult::UnInitialized;
        }
        debug!("disconnected cb set");
        self.device_event_cb.lock().unwrap().replace(cb);
        WootingAnalogResult::Ok
    }

    fn clear_device_event_cb(&mut self) -> WootingAnalogResult {
        if !self.initialised {
            return WootingAnalogResult::UnInitialized;
        }

        debug!("disconnected cb cleared");
        self.device_event_cb.lock().unwrap().take();
        WootingAnalogResult::Ok
    }

    fn read_analog(&mut self, code: u16, device_id: DeviceID) -> SDKResult<f32> {
        if !self.initialised {
            return WootingAnalogResult::UnInitialized.into();
        }

        if self.devices.lock().unwrap().is_empty() {
            return WootingAnalogResult::NoDevices.into();
        }

        //If the Device ID is 0 we want to go through all the connected devices
        //and combine the analog values
        if device_id == 0 {
            let mut analog: f32 = -1.0;
            let mut error: WootingAnalogResult = WootingAnalogResult::Ok;
            let mut dc = Vec::new();
            for (id, device) in self.devices.lock().unwrap().iter_mut() {
                match device.read_analog(code).into() {
                    Ok(val) => {
                        analog = analog.max(val);
                    }
                    Err(WootingAnalogResult::DeviceDisconnected) => {
                        dc.push(*id);
                        error = WootingAnalogResult::DeviceDisconnected;
                    }
                    Err(e) => {
                        error = e;
                    }
                }
            }
            for dev in dc.drain(..) {
                let device = self.devices.lock().unwrap().remove(&dev).unwrap();
                handle_device_event(self.device_event_cb.lock().unwrap().borrow(), &device, DeviceEventType::Disconnected);
            }

            if analog < 0.0 {
                error.into()
            } else {
                analog.into()
            }
        } else
        //If the device id is not 0, we try and find a connected device with that ID and read from it
        {
            let mut disconnected = false;
            let ret = match self.devices.lock().unwrap().get_mut(&device_id) {
                Some(device) => match device.read_analog(code).into() {
                    Ok(val) => val.into(),
                    Err(WootingAnalogResult::DeviceDisconnected) => {
                        disconnected = true;
                        WootingAnalogResult::DeviceDisconnected.into()
                    }
                    Err(e) => Err(e).into(),
                },
                None => WootingAnalogResult::NoDevices.into(),
            };
            if disconnected {
                let dev = self.devices.lock().unwrap().remove(&device_id).unwrap();
                handle_device_event(self.device_event_cb.lock().unwrap().borrow(), &dev, DeviceEventType::Disconnected);
            }

            ret
        }
    }

    fn read_full_buffer(
        &mut self,
        max_length: usize,
        device_id: DeviceID,
    ) -> SDKResult<HashMap<c_ushort, c_float>> {
        if !self.initialised {
            return WootingAnalogResult::UnInitialized.into();
        }

        if self.devices.lock().unwrap().is_empty() {
            return WootingAnalogResult::NoDevices.into();
        }

        //If the Device ID is 0 we want to go through all the connected devices
        //and combine the analog values
        if device_id == 0 {
            let mut analog: HashMap<c_ushort, c_float> = HashMap::new();
            let mut any_read = false;
            let mut error: WootingAnalogResult = WootingAnalogResult::Ok;
            let mut dc = Vec::new();
            for (id, device) in self.devices.lock().unwrap().iter_mut() {
                match device.read_full_buffer(max_length).into() {
                    Ok(val) => {
                        any_read = true;
                        analog.extend(val);
                    }
                    Err(WootingAnalogResult::DeviceDisconnected) => {
                        dc.push(*id);
                        error = WootingAnalogResult::DeviceDisconnected;
                    }
                    Err(e) => {
                        error = e;
                    }
                }
            }
            for dev in dc.drain(..) {
                let device = self.devices.lock().unwrap().remove(&dev).unwrap();
                handle_device_event(self.device_event_cb.lock().unwrap().borrow(), &device, DeviceEventType::Disconnected);
            }

            if !any_read {
                error.into()
            } else {
                Ok(analog).into()
            }
        } else
        //If the device id is not 0, we try and find a connected device with that ID and read from it
        {
            let mut disconnected = false;
            let ret = match self.devices.lock().unwrap().get_mut(&device_id) {
                Some(device) => match device.read_full_buffer(max_length).into() {
                    Ok(val) => Ok(val).into(),
                    Err(WootingAnalogResult::DeviceDisconnected) => {
                        disconnected = true;
                        WootingAnalogResult::DeviceDisconnected.into()
                    }
                    Err(e) => Err(e).into(),
                },
                None => WootingAnalogResult::NoDevices.into(),
            };
            if disconnected {
                let dev = self.devices.lock().unwrap().remove(&device_id).unwrap();
                handle_device_event(self.device_event_cb.lock().unwrap().borrow(), &dev, DeviceEventType::Disconnected);
            }

            ret
        }
    }

    fn device_info(&mut self, buffer: &mut [DeviceInfoPointer]) -> SDKResult<c_int> {
        if !self.initialised {
            return WootingAnalogResult::UnInitialized.into();
        }

        let mut count = 0;
        for (_id, device) in self.devices.lock().unwrap().iter() {
            buffer[count] = device.device_info.clone();
            count += 1;
        }
        (count as c_int).into()
    }
}

declare_plugin!(WootingPlugin, WootingPlugin::new);
