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
use crate::xhci::context::EndpointContext;
use crate::xhci::context::InputContext;
use crate::xhci::context::InputControlContext;
use crate::xhci::future::TransferEventFuture;
use crate::xhci::ring::CommandRing;
use crate::xhci::ring::TransferRing;
use crate::xhci::trb::GenericTrbEntry;
use crate::xhci::EndpointType;
use crate::xhci::Xhci;
use alloc::format;
use alloc::vec::Vec;
use core::cmp::max;
use core::pin::Pin;

pub async fn init_usb_hid_keyboard(
    xhci: &mut Xhci,
    port: usize,
    slot: u8,
    input_context: &mut Pin<&mut InputContext>,
    ctrl_ep_ring: &mut CommandRing,
    descriptors: &Vec<UsbDescriptor>,
) -> Result<[Option<TransferRing>; 32]> {
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

    let portsc = xhci.portsc(port)?;
    let mut input_ctrl_ctx = InputControlContext::default();
    input_ctrl_ctx.add_context(0)?;
    const EP_RING_NONE: Option<TransferRing> = None;
    let mut ep_rings = [EP_RING_NONE; 32];
    let mut last_dci = 1;
    for ep_desc in ep_desc_list {
        match EndpointType::from(&ep_desc) {
            EndpointType::InterruptIn => {
                let tring = TransferRing::new(8)?;
                input_ctrl_ctx.add_context(ep_desc.dci())?;
                input_context.set_ep_ctx(
                    ep_desc.dci(),
                    EndpointContext::new_interrupt_in_endpoint(
                        portsc.max_packet_size()?,
                        tring.ring_phys_addr(),
                        portsc.port_speed(),
                        ep_desc.interval,
                        8,
                    )?,
                )?;
                last_dci = max(last_dci, ep_desc.dci());
                ep_rings[ep_desc.dci()] = Some(tring);
            }
            _ => {
                println!("Ignoring {:?}", ep_desc);
            }
        }
    }
    input_context.set_last_valid_dci(last_dci)?;
    input_context.set_input_ctrl_ctx(input_ctrl_ctx)?;
    let cmd = GenericTrbEntry::cmd_configure_endpoint(input_context.as_ref(), slot);
    xhci.send_command(cmd).await?.completed()?;
    xhci.request_set_config(slot, ctrl_ep_ring, config_desc.config_value())
        .await?;
    xhci.request_set_interface(
        slot,
        ctrl_ep_ring,
        interface_desc.interface_number(),
        interface_desc.alt_setting(),
    )
    .await?;
    xhci.request_set_protocol(
        slot,
        ctrl_ep_ring,
        interface_desc.interface_number(),
        0, /* Boot Protocol */
    )
    .await?;
    // 4.6.6 Configure Endpoint
    // When configuring or deconfiguring a device, only after completing a successful
    // Configure Endpoint Command and a successful USB SET_CONFIGURATION
    // request may software schedule data transfers through a newly enabled endpoint
    // or Stream Transfer Ring of the Device Slot.
    for (dci, tring) in ep_rings.iter_mut().enumerate() {
        match tring {
            Some(tring) => {
                tring.fill_ring()?;
                xhci.notify_ep(slot, dci);
            }
            None => {}
        }
    }
    Ok(ep_rings)
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
        4..=30 => Ok(KeyEvent::Char((b'a' + usage_id - 4) as char)),
        40 => Ok(KeyEvent::Enter),
        _ => Err(Error::FailedString(format!(
            "Unhandled USB HID Keyboard Usage ID {usage_id:}"
        ))),
    }
}

pub async fn attach_usb_device(
    xhci: &mut Xhci,
    port: usize,
    slot: u8,
    input_context: &mut Pin<&mut InputContext>,
    ctrl_ep_ring: &mut CommandRing,
    descriptors: &Vec<UsbDescriptor>,
) -> Result<()> {
    let mut ep_rings =
        init_usb_hid_keyboard(xhci, port, slot, input_context, ctrl_ep_ring, descriptors).await?;

    let portsc = xhci.portsc(port)?;
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
                if let Some(ref mut tring) = ep_rings[trb.dci()] {
                    tring.dequeue_trb(transfer_trb_ptr)?;
                    xhci.notify_ep(slot, trb.dci());
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
