extern crate alloc;

use crate::allocator::ALLOCATOR;
use crate::arch::x86_64::paging::disable_cache;
use crate::arch::x86_64::paging::IoBox;
use crate::error::Result;
use crate::error::WasabiError;
use crate::xhci::error_stringify;
use crate::xhci::trb::GenericTrbEntry;
use crate::xhci::trb::NormalTrb;
use crate::xhci::trb::TrbType;
use crate::xhci::EventRingSegmentTableEntry;
use alloc::alloc::Layout;
use core::marker::PhantomPinned;
use core::mem::size_of;
use core::ptr::null_mut;
use core::ptr::read_volatile;
use core::ptr::write_volatile;

#[derive(Debug)]
#[repr(C, align(4096))]
pub struct TrbRing {
    trb: [GenericTrbEntry; Self::NUM_TRB],
    current_index: usize,
    _pinned: PhantomPinned,
}
// Limiting the size of TrbRing to be equal or less than 4096
// to avoid crossing 64KiB boundaries. See Table 6-1 of xhci spec.
const _: () = assert!(size_of::<TrbRing>() <= 4096);
impl TrbRing {
    pub const NUM_TRB: usize = 16;
    fn new() -> IoBox<Self> {
        IoBox::new()
    }
    pub fn phys_addr(&self) -> u64 {
        &self.trb[0] as *const GenericTrbEntry as u64
    }
    pub const fn num_trbs(&self) -> usize {
        Self::NUM_TRB
    }
    fn advance_index(&mut self, new_cycle: bool) -> Result<()> {
        if self.current().cycle_state() == new_cycle {
            return Err(WasabiError::Failed("cycle state does not change"));
        }
        self.trb[self.current_index].set_cycle_state(new_cycle);
        self.current_index = (self.current_index + 1) % self.trb.len();
        Ok(())
    }
    fn advance_index_notoggle(&mut self, cycle_ours: bool) -> Result<()> {
        if self.current().cycle_state() != cycle_ours {
            return Err(WasabiError::Failed("cycle state mismatch"));
        }
        self.current_index = (self.current_index + 1) % self.trb.len();
        Ok(())
    }
    fn current(&self) -> GenericTrbEntry {
        self.trb(self.current_index)
    }
    fn current_index(&self) -> usize {
        self.current_index
    }
    fn current_ptr(&self) -> usize {
        &self.trb[self.current_index] as *const GenericTrbEntry as usize
    }
    fn trb(&self, index: usize) -> GenericTrbEntry {
        unsafe { read_volatile(&self.trb[index]) }
    }
    fn trb_ptr(&self, index: usize) -> usize {
        &self.trb[index] as *const GenericTrbEntry as usize
    }
    fn write(&mut self, index: usize, trb: GenericTrbEntry) -> Result<()> {
        if index < self.trb.len() {
            unsafe {
                write_volatile(&mut self.trb[index], trb);
            }
            Ok(())
        } else {
            Err(WasabiError::Failed("TrbRing Out of Range"))
        }
    }
    fn write_current(&mut self, trb: GenericTrbEntry) {
        self.write(self.current_index, trb)
            .expect("writing to the current index shall not fail")
    }
}

pub struct CommandRing {
    ring: IoBox<TrbRing>,
    cycle_state_ours: bool,
}
impl Default for CommandRing {
    fn default() -> Self {
        let mut this = Self {
            ring: TrbRing::new(),
            cycle_state_ours: false,
        };
        let link_trb = GenericTrbEntry::trb_link(this.ring.as_ref());
        unsafe { this.ring.get_unchecked_mut() }
            .write(TrbRing::NUM_TRB - 1, link_trb)
            .expect("failed to write a link trb");
        this
    }
}
impl CommandRing {
    pub fn ring_phys_addr(&self) -> u64 {
        self.ring.as_ref() as *const TrbRing as u64
    }
    pub fn push(&mut self, mut src: GenericTrbEntry) -> Result<u64> {
        // Calling get_unchecked_mut() here is safe
        // as far as this function does not move the ring out.
        let ring = unsafe { self.ring.get_unchecked_mut() };
        if ring.current().cycle_state() != self.cycle_state_ours {
            return Err(WasabiError::Failed("Command Ring is Full"));
        }
        src.set_cycle_state(self.cycle_state_ours);
        let dst_ptr = ring.current_ptr();
        ring.write_current(src);
        ring.advance_index(!self.cycle_state_ours)?;
        if ring.current().trb_type() == TrbType::Link as u32 {
            // Reached to Link TRB. Let's skip it and toggle the cycle.
            ring.advance_index(!self.cycle_state_ours)?;
            self.cycle_state_ours = !self.cycle_state_ours;
        }
        // The returned ptr will be used for waiting on command completion events.
        Ok(dst_ptr as u64)
    }
}

// Producer: Software
// Consumer: xHC
// Producer is responsible to flip the cycle bits
pub struct TransferRing {
    ring: IoBox<TrbRing>,
    cycle_state_ours: bool,
    // enqeue_index: usize, // will be maintained by .ring
    dequeue_index: usize,
    buffers: [*mut u8; TrbRing::NUM_TRB - 1],
}
impl TransferRing {
    const BUF_SIZE: usize = 4096;
    const BUF_ALIGN: usize = 4096;
    pub fn new() -> Result<Self> {
        let mut this = Self {
            ring: TrbRing::new(),
            cycle_state_ours: false,
            dequeue_index: 0,
            buffers: [null_mut(); TrbRing::NUM_TRB - 1],
        };
        // Fill all TRBs but keep them owned by us
        let link_trb = GenericTrbEntry::trb_link(this.ring.as_ref());
        let num_trbs = this.ring.as_ref().num_trbs();
        let mut_ring = unsafe { this.ring.get_unchecked_mut() };
        mut_ring
            .write(num_trbs - 1, link_trb)
            .expect("failed to write a link trb");
        for (i, v) in this.buffers.iter_mut().enumerate() {
            *v = ALLOCATOR.alloc_with_options(
                Layout::from_size_align(Self::BUF_SIZE, Self::BUF_ALIGN)
                    .map_err(error_stringify)?,
            );
            mut_ring
                .write(i, NormalTrb::new(*v, 8).into())
                .expect("failed to write a link trb");
        }
        Ok(this)
    }
    pub fn fill_ring(&mut self) -> Result<()> {
        // 4.9.2.2 Pointer Advancement
        // To prevent overruns, software shall determine when the Ring is full. The ring is
        // defined as “full” if advancing the Enqueue Pointer will make it equal to the
        // Dequeue Pointer.
        //
        // Note: without taking care of this, QEMU will work without errors but not on the real
        // hardwares...
        loop {
            // Wrap with num_trbs() - 1 to ignore LinkTrb
            let next_enqueue_index =
                (self.ring.as_ref().current_index() + 1) % (self.ring.as_ref().num_trbs() - 1);
            if next_enqueue_index == self.dequeue_index {
                // Ring is full. stop filling.
                break;
            }
            let mut_ring = unsafe { self.ring.get_unchecked_mut() };
            mut_ring.advance_index(!self.cycle_state_ours)?;
        }
        Ok(())
    }
    pub fn dequeue_trb(&mut self, trb_ptr: usize) -> Result<()> {
        // Update dequeue_index
        if self.ring.as_ref().trb_ptr(self.dequeue_index) != trb_ptr {
            return Err(WasabiError::Failed("unexpected trb ptr"));
        }
        // Wrap with num_trbs() - 1 to ignore LinkTrb
        self.dequeue_index = (self.dequeue_index + 1) % (self.ring.as_ref().num_trbs() - 1);
        // Update enqeue_index
        let mut_ring = unsafe { self.ring.get_unchecked_mut() };
        mut_ring.advance_index(!self.cycle_state_ours)?;
        if mut_ring.current().trb_type() == TrbType::Link as u32 {
            // Reached to Link TRB. Let's skip it and toggle the cycle.
            mut_ring.advance_index(!self.cycle_state_ours)?;
            self.cycle_state_ours = !self.cycle_state_ours;
        }
        Ok(())
    }
    pub fn ring_phys_addr(&self) -> u64 {
        self.ring.as_ref() as *const TrbRing as u64
    }
}

pub struct EventRing {
    ring: IoBox<TrbRing>,
    erst: IoBox<EventRingSegmentTableEntry>,
    cycle_state_ours: bool,
    erdp: Option<*mut u64>,
}
impl EventRing {
    pub fn new() -> Result<Self> {
        let ring = TrbRing::new();
        let erst = EventRingSegmentTableEntry::new(&ring)?;
        disable_cache(&erst);

        Ok(Self {
            ring,
            erst,
            cycle_state_ours: true,
            erdp: None,
        })
    }
    pub fn set_erdp(&mut self, erdp: *mut u64) {
        self.erdp = Some(erdp);
    }
    pub fn ring_phys_addr(&self) -> u64 {
        self.ring.as_ref() as *const TrbRing as u64
    }
    pub fn erst_phys_addr(&self) -> u64 {
        self.erst.as_ref() as *const EventRingSegmentTableEntry as u64
    }
    fn has_next_event(&self) -> bool {
        self.ring.as_ref().current().cycle_state() == self.cycle_state_ours
    }
    pub fn pop(&mut self) -> Result<Option<GenericTrbEntry>> {
        if !self.has_next_event() {
            return Ok(None);
        }
        let e = self.ring.as_ref().current();
        let eptr = self.ring.as_ref().current_ptr() as u64;
        unsafe { self.ring.get_unchecked_mut() }.advance_index_notoggle(self.cycle_state_ours)?;
        unsafe {
            let erdp = self.erdp.expect("erdp is not set");
            write_volatile(erdp, eptr | (*erdp & 0b1111));
        }
        if self.ring.as_ref().current_index() == 0 {
            self.cycle_state_ours = !self.cycle_state_ours;
        }
        Ok(Some(e))
    }
}