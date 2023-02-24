extern crate alloc;

use crate::boot_info::BootInfo;
use crate::error::Result;
use crate::memory::alloc_pages;
use crate::println;
use crate::util::PAGE_SIZE;
use crate::x86_64::read_cr2;
use alloc::boxed::Box;
use attr_bits::BIT_FLAGS_INTGATE;
use attr_bits::BIT_FLAGS_PRESENT;
use core::arch::asm;
use core::arch::global_asm;
use core::fmt;
use core::mem::size_of;
use core::pin::Pin;

// System V AMD64 (sysv64) ABI:
//   args: RDI, RSI, RDX, RCX, R8, R9
//   callee-saved: RBX, RBP, R12, R13, R14, R15
//   caller-saved: otherwise

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy)]
struct FPUContenxt {
    data: [u8; 512],
}
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy)]
struct GeneralRegisterContext {
    rax: u64,
    rdx: u64,
    rbx: u64,
    rbp: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rcx: u64,
}
const _: () = assert!(size_of::<GeneralRegisterContext>() == (16 - 1) * 8);
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct InterruptContext {
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}
const _: () = assert!(size_of::<InterruptContext>() == 8 * 5);
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy)]
struct InterruptInfo {
    // This struct is placed at top of the interrupt stack.
    // Should be aligned on 16-byte boundaries to pass the
    // alignment checks done by FXSAVE / FXRSTOR
    fpu_context: FPUContenxt, // used by FXSAVE / FXRSTOR
    _dummy: u64,
    greg: GeneralRegisterContext,
    error_code: u64,
    ctx: InterruptContext,
}
const _: () = assert!(size_of::<InterruptInfo>() == (16 + 4 + 1) * 8 + 8 + 512);
impl fmt::Debug for InterruptInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "
        {{
            rip: {:#018X}, CS: {:#06X},
            rsp: {:#018X}, SS: {:#06X},
            rbp: {:#018X},

            rflags:     {:#018X},
            error_code: {:#018X},

            rax: {:#018X}, rcx: {:#018X},
            rdx: {:#018X}, rbx: {:#018X},
            rsi: {:#018X}, rdi: {:#018X},
            r8:  {:#018X}, r9:  {:#018X},
            r10: {:#018X}, r11: {:#018X},
            r12: {:#018X}, r13: {:#018X},
            r14: {:#018X}, r15: {:#018X},
        }}",
            self.ctx.rip,
            self.ctx.cs,
            self.ctx.rsp,
            self.ctx.ss,
            self.greg.rbp,
            self.ctx.rflags,
            self.error_code,
            //
            self.greg.rax,
            self.greg.rcx,
            self.greg.rdx,
            self.greg.rbx,
            //
            self.greg.rsi,
            self.greg.rdi,
            //
            self.greg.r8,
            self.greg.r9,
            self.greg.r10,
            self.greg.r11,
            self.greg.r12,
            self.greg.r13,
            self.greg.r14,
            self.greg.r15,
        )
    }
}

// SDM Vol.3: 6.14.2 64-Bit Mode Stack Frame
// In IA-32e mode, the RSP is aligned to a 16-byte boundary
// before pushing the stack frame

/// This generates interrupt_entrypointN()
/// Generated asm will be looks like this:
/// ```
/// .global interrupt_entrypointN
///    interrupt_entrypointN:
///    push 0 // No error code
///    push rcx // Save rcx first to reuse
///    mov rcx, N // INT#
///    jmp inthandler_common
/// ```
macro_rules! interrupt_entrypoint {
    ($index:literal) => {
        global_asm!(concat!(
            ".global interrupt_entrypoint",
            stringify!($index),
            "\n",
            "interrupt_entrypoint",
            stringify!($index),
            ":\n",
            "push 0 // No error code\n",
            "push rcx // Save rcx first to reuse\n",
            "mov rcx, ",
            stringify!($index),
            "\n",
            "jmp inthandler_common"
        ));
    };
}
macro_rules! interrupt_entrypoint_with_ecode {
    ($index:literal) => {
        global_asm!(concat!(
            ".global interrupt_entrypoint",
            stringify!($index),
            "\n",
            "interrupt_entrypoint",
            stringify!($index),
            ":\n",
            "push rcx // Save rcx first to reuse\n",
            "mov rcx, ",
            stringify!($index),
            "\n",
            "jmp inthandler_common"
        ));
    };
}

interrupt_entrypoint!(3);
interrupt_entrypoint!(6);
interrupt_entrypoint_with_ecode!(13);
interrupt_entrypoint_with_ecode!(14);
interrupt_entrypoint!(32);

extern "sysv64" {
    fn interrupt_entrypoint3();
    fn interrupt_entrypoint6();
    fn interrupt_entrypoint13();
    fn interrupt_entrypoint14();
    fn interrupt_entrypoint32();
}

global_asm!(
    r#"
.global inthandler_common
inthandler_common:
    // General purpose registers (except rsp and rcx)
    mov ax,16 // KERNEL_DS == 2 << 3
    mov es,ax
    mov ds,ax
    mov fs,ax
    mov gs,ax
    mov ss,ax
    push r15
    push r14
    push r13
    push r12
    push r11
    push r10
    push r9
    push r8
    push rdi
    push rsi
    push rbp
    push rbx
    push rdx
    push rax
    // FPU State
    sub rsp, 512 + 8
    fxsave64[rsp]
    // 1st parameter: pointer to the saved CPU state
    mov rdi, rsp
    // Align the stack to 16-bytes boundary
    mov rbp, rsp
    and rsp, -16
    // 2nd parameter: Int#
    mov rsi, rcx

    call inthandler

    mov rsp, rbp
    //
    fxrstor64[rsp]
    add rsp, 512 + 8
    //
    pop rax
    pop rdx
    pop rbx
    pop rbp
    pop rsi
    pop rdi
    pop r8
    pop r9
    pop r10
    pop r11
    pop r12
    pop r13
    pop r14
    pop r15
    //
    pop rcx
    add rsp, 8 // for Error Code
    iretq
"#
);

#[no_mangle]
extern "sysv64" fn inthandler(info: &InterruptInfo, index: usize) {
    if index == 32 {
        let bsp_local_apic = BootInfo::take().bsp_local_apic();
        bsp_local_apic.notify_end_of_interrupt();
        return;
    }
    println!("Interrupt Info: {:?}", info);
    match index {
        3 => {
            println!("Exception {index:#04X}: Breakpoint");
        }
        6 => {
            println!("Exception {index:#04X}: Invalid Opcode");
        }
        14 => {
            println!("Exception {index:#04X}: Page Fault");
            println!("CR2={:#018X}", read_cr2());
            println!(
                "Caused by: {} mode {} access to a {} page",
                if info.error_code & 0b0100 != 0 {
                    "user"
                } else {
                    "supervisor"
                },
                if info.error_code & 0b0010 != 0 {
                    "write"
                } else {
                    "read"
                },
                if info.error_code & 0b0001 != 0 {
                    "present"
                } else {
                    "non-present"
                },
            );
        }
        _ => {
            println!("Exception {index:#04X}: Not handled");
        }
    }
    panic!("fatal exception");
}

#[no_mangle]
extern "sysv64" fn int_handler_unimplemented() {
    panic!("unexpected interrupt!");
}

mod attr_bits {
    // PDDRTTTT (TTTT: type, R: reserved, D: DPL, P: present)
    pub const BIT_FLAGS_INTGATE: u8 = 0b0000_1110u8;
    pub const BIT_FLAGS_PRESENT: u8 = 0b1000_0000u8;
}

#[repr(u8)]
#[derive(Copy, Clone)]
enum IdtAttr {
    // Without _NotPresent value, MaybeUninit::zeroed() on
    // this struct will be undefined behavior.
    _NotPresent = 0,
    IntGateDPL0 = BIT_FLAGS_INTGATE | BIT_FLAGS_PRESENT,
}

#[repr(packed)]
#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct IdtDescriptor {
    offset_low: u16,
    segment_selector: u16,
    ist_index: u8,
    attr: IdtAttr,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}
const _: () = assert!(size_of::<IdtDescriptor>() == 16);
impl IdtDescriptor {
    fn new(
        segment_selector: u16,
        ist_index: u8,
        attr: IdtAttr,
        f: unsafe extern "sysv64" fn(),
    ) -> Self {
        let handler_addr = f as *const unsafe extern "sysv64" fn() as usize;
        Self {
            offset_low: handler_addr as u16,
            offset_mid: (handler_addr >> 16) as u16,
            offset_high: (handler_addr >> 32) as u32,
            segment_selector,
            ist_index,
            attr,
            _reserved: 0,
        }
    }
}

#[allow(dead_code)]
#[repr(packed)]
struct IdtrParameters<'a> {
    limit: u16,
    base: &'a [IdtDescriptor; 0x100],
}

pub struct Idt {
    entries: [IdtDescriptor; 0x100],
}
impl Idt {
    pub fn new(segment_selector: u16) -> Result<Pin<Box<Self>>> {
        let mut idt = Idt {
            entries: [IdtDescriptor::new(
                segment_selector,
                1,
                IdtAttr::IntGateDPL0,
                int_handler_unimplemented,
            ); 0x100],
        };
        idt.entries[3] = IdtDescriptor::new(
            segment_selector,
            1,
            IdtAttr::IntGateDPL0,
            interrupt_entrypoint3,
        );
        idt.entries[6] = IdtDescriptor::new(
            segment_selector,
            1,
            IdtAttr::IntGateDPL0,
            interrupt_entrypoint6,
        );
        idt.entries[13] = IdtDescriptor::new(
            segment_selector,
            1,
            IdtAttr::IntGateDPL0,
            interrupt_entrypoint13,
        );
        idt.entries[14] = IdtDescriptor::new(
            segment_selector,
            1,
            IdtAttr::IntGateDPL0,
            interrupt_entrypoint14,
        );
        idt.entries[32] = IdtDescriptor::new(
            segment_selector,
            1,
            IdtAttr::IntGateDPL0,
            interrupt_entrypoint32,
        );
        let idt = Box::pin(idt);
        let params = IdtrParameters {
            limit: size_of::<Self>() as u16 - 1,
            base: &idt.entries,
        };
        println!("Loading IDT @ {:#018X}", params.base.as_ptr() as u64);
        // SAFETY: This is safe since it loads a valid IDT that is constructed in the code just above
        unsafe {
            asm!("lidt [rcx]",
                in("rcx") &params);
        }
        Ok(idt)
    }
}

// 7.7 TASK MANAGEMENT IN 64-BIT MODE
#[repr(packed)]
struct TaskStateSegment64Inner {
    _reserved0: u32,
    _rsp: [u64; 3], // for switch into ring0-2
    _ist: [u64; 8], // ist[1]~ist[7] (ist[0] is reserved)
    _reserved1: [u16; 5],
    _io_map_base_addr: u16,
}
const _: () = assert!(size_of::<TaskStateSegment64Inner>() == 104);

pub struct TaskStateSegment64 {
    tss64: TaskStateSegment64Inner,
    _stack_for_ring0: Pin<Box<[u8]>>,
}
impl TaskStateSegment64 {
    pub fn phys_addr(&self) -> u64 {
        &self.tss64 as *const TaskStateSegment64Inner as u64
    }
    pub fn new() -> Result<Pin<Box<Self>>> {
        const RING0_STACK_NUM_PAGES: usize = 16;
        let stack_for_ring0 = alloc_pages(RING0_STACK_NUM_PAGES)?;
        let rsp0 = unsafe {
            stack_for_ring0
                .as_ptr()
                .add(RING0_STACK_NUM_PAGES * PAGE_SIZE) as u64
        };
        let tss64 = TaskStateSegment64Inner {
            _reserved0: 0,
            _rsp: [rsp0, 0, 0],
            _ist: [rsp0; 8],
            _reserved1: [0; 5],
            _io_map_base_addr: 0,
        };
        let this = Box::pin(Self {
            tss64,
            _stack_for_ring0: stack_for_ring0,
        });
        println!(
            "TSS64 created @ {:#p}, with rsp0 = {:#018X}",
            this.as_ref().get_ref(),
            rsp0
        );
        Ok(this)
    }
}
impl Drop for TaskStateSegment64 {
    fn drop(&mut self) {
        panic!("TSS64 being dropped!");
    }
}
