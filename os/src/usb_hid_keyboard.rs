extern crate alloc;

use crate::bitset::BitSet;
use crate::error::Error;
use crate::error::Result;
use crate::input::InputManager;
use crate::memory::Mmio;
use crate::println;
use crate::usb::ConfigDescriptor;
use crate::usb::EndpointDescriptor;
use crate::usb::InterfaceDescriptor;
use crate::usb::UsbDescriptor;
use crate::xhci::device::UsbDeviceDriverContext;
use crate::xhci::device::UsbHidProtocol;
use crate::xhci::future::TransferEventFuture;
use alloc::format;
use alloc::vec::Vec;

pub fn pick_config(
    descriptors: &Vec<UsbDescriptor>,
) -> Result<(
    ConfigDescriptor,
    InterfaceDescriptor,
    Vec<EndpointDescriptor>,
)> {
    let mut last_config: Option<ConfigDescriptor> = None;
    let mut boot_keyboard_interface: Option<InterfaceDescriptor> = None;
    let mut ep_desc_list: Vec<EndpointDescriptor> = Vec::new();
    for d in descriptors {
        match d {
            UsbDescriptor::Config(e) => {
                if boot_keyboard_interface.is_some() {
                    break;
                }
                last_config = Some(*e);
                ep_desc_list.clear();
            }
            UsbDescriptor::Interface(e) => {
                if let (3, 1, 1) = e.triple() {
                    boot_keyboard_interface = Some(*e)
                }
            }
            UsbDescriptor::Endpoint(e) => {
                ep_desc_list.push(*e);
            }
            _ => {}
        }
    }
    let config_desc = last_config.ok_or(Error::Failed("No USB KBD Boot config found"))?;
    let interface_desc =
        boot_keyboard_interface.ok_or(Error::Failed("No USB KBD Boot interface found"))?;
    Ok((config_desc, interface_desc, ep_desc_list))
}

pub async fn init_usb_hid_keyboard(ddc: &mut UsbDeviceDriverContext) -> Result<()> {
    let descriptors = ddc.descriptors();
    let (config_desc, interface_desc, ep_desc_list) = pick_config(descriptors)?;
    for ep_desc in &ep_desc_list {
        println!("usb_hid_kbd: EP: {ep_desc:?}")
    }
    ddc.set_config(config_desc.config_value()).await?;
    ddc.set_interface(&interface_desc).await?;
    ddc.set_protocol(&interface_desc, UsbHidProtocol::BootProtocol)
        .await?;
    // 4.6.6 Configure Endpoint
    // When configuring or deconfiguring a device, only after completing a successful
    // Configure Endpoint Command and a successful USB SET_CONFIGURATION
    // request may software schedule data transfers through a newly enabled endpoint
    // or Stream Transfer Ring of the Device Slot.
    for ep_desc in &ep_desc_list {
        let ep_ring = ddc
            .ep_ring(ep_desc.dci())?
            .as_ref()
            .ok_or(Error::Failed("Endpoint not created"))?;
        ep_ring.fill_ring()?;
        ddc.notify_ep(ep_desc)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum KeyEvent {
    None,
    Char(char),
    Enter,
}

impl KeyEvent {
    fn to_char(&self) -> Option<char> {
        match self {
            KeyEvent::Char(c) => Some(*c),
            KeyEvent::Enter => Some('\n'),
            _ => None,
        }
    }
}

fn usage_id_to_char(usage_id: u8) -> Result<KeyEvent> {
    match usage_id {
        0 => Ok(KeyEvent::None),
        4..=29 => Ok(KeyEvent::Char((b'a' + usage_id - 4) as char)),
        30..=39 => Ok(KeyEvent::Char((b'0' + (usage_id + 1) % 10) as char)),
        40 => Ok(KeyEvent::Enter),
        _ => Err(Error::FailedString(format!(
            "Unhandled USB HID Keyboard Usage ID {usage_id:}"
        ))),
    }
}

pub async fn attach_usb_device(mut ddc: UsbDeviceDriverContext) -> Result<()> {
    init_usb_hid_keyboard(&mut ddc).await?;

    let port = ddc.port();
    let slot = ddc.slot();
    let xhci = ddc.xhci();
    let portsc = xhci.portsc(port)?.upgrade().ok_or("PORTSC was invalid")?;
    let mut prev_pressed_keys = BitSet::<32>::new();
    loop {
        let event_trb = TransferEventFuture::new_on_slot(xhci.primary_event_ring(), slot).await;
        match event_trb {
            Ok(Some(trb)) => {
                let transfer_trb_ptr = trb.data() as usize;
                let mut report = [0u8; 8];
                report.copy_from_slice(
                    unsafe {
                        Mmio::<[u8; 8]>::from_raw(
                            *(transfer_trb_ptr as *const usize) as *mut [u8; 8],
                        )
                    }
                    .as_ref(),
                );
                if let Some(ref mut tring) = ddc.ep_ring(trb.dci())?.as_ref() {
                    tring.dequeue_trb(transfer_trb_ptr)?;
                    xhci.notify_ep(slot, trb.dci())?;
                }
                let mut next_pressed_keys = BitSet::<32>::new();
                // First two bytes are modifiers, so skip them
                let keycodes = report.iter().skip(2);
                for value in keycodes {
                    next_pressed_keys.insert(*value as usize).unwrap();
                }
                let change = prev_pressed_keys.symmetric_difference(&next_pressed_keys);
                for id in change.iter() {
                    let c = usage_id_to_char(id as u8);
                    if let Ok(c) = c {
                        if !prev_pressed_keys.get(id).unwrap_or(false) {
                            // the key state was changed from released to pressed
                            if c == KeyEvent::None {
                                continue;
                            }
                            if let Some(c) = c.to_char() {
                                InputManager::take().push_input(c);
                            }
                        }
                    } else {
                        println!("{c:?}");
                    }
                }
                prev_pressed_keys = next_pressed_keys;
            }
            Ok(None) => {
                // Timed out. Do nothing.
            }
            Err(e) => {
                println!("e: {:?}", e);
            }
        }
        if !portsc.ccs() {
            return Err(Error::FailedString(format!("port {} disconnected", port)));
        }
    }
}
