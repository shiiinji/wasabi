use crate::error::Result;
use crate::executor::yield_execution;
use crate::mutex::Mutex;
use core::arch::asm;
use core::arch::global_asm;
use core::mem::size_of;
use core::mem::MaybeUninit;

pub static CONTEXT_OS: Mutex<ExecutionContext> =
    Mutex::new(ExecutionContext::default(), "CONTEXT_OS");
pub static CONTEXT_APP: Mutex<ExecutionContext> =
    Mutex::new(ExecutionContext::default(), "CONTEXT_APP");

#[repr(C)]
#[derive(Clone, Debug)]
pub struct ExecutionContext {
    pub fpu: FpuContext,
    pub cpu: CpuContext,
    // CpuContext should be at the end to put rsp at bottom
}
impl ExecutionContext {
    /// # Safety
    /// This function should only be used for passing the ptr
    /// to the context-switching asm macro so that they can
    /// access to the context without taking a lock.
    /// The context-switching code does not modify the context
    /// while the asm code is running, so the borrow rule for
    /// the object will be maintained from the Rust's point of view.
    pub unsafe fn as_mut_ptr(&mut self) -> *mut Self {
        self as *mut Self
    }
    // We implement this outside of trait since Default trait is not const yet.
    // c.f. https://github.com/rust-lang/rust/issues/67792
    pub const fn default() -> Self {
        Self {
            fpu: FpuContext { data: [0u8; 512] },
            cpu: CpuContext::default(),
        }
    }
}
const _: () =
    assert!(size_of::<ExecutionContext>() == size_of::<FpuContext>() + size_of::<CpuContext>());

#[repr(C, align(16))]
#[derive(Clone, Debug)]
pub struct FpuContext {
    // See manual for FXSAVE and FXRSTOR
    // Should be aligned on 16-byte boundary
    pub data: [u8; 512],
}
const _: () = assert!(size_of::<FpuContext>() == 512);

#[repr(C)]
#[derive(Clone, Debug)]
pub struct CpuContext {
    pub rip: u64,    // set by CPU
    pub rflags: u64, // set by CPU
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rsp: u64, // rsp should be here to make load / store easy
}
impl CpuContext {
    // We implement this outside of trait since Default trait is not const yet.
    // c.f. https://github.com/rust-lang/rust/issues/67792
    const fn default() -> Self {
        // SAFETY: CpuContext only contains integers so zeroed out structure is completely valid as
        // this type.
        unsafe { MaybeUninit::zeroed().assume_init() }
    }
}
const _: () = assert!(size_of::<CpuContext>() == 8 * 16 + 8 * 2);

pub async fn exec_app_context() -> Result<i64> {
    let mut retcode: i64;
    loop {
        let mut exit_reason: i64;
        unsafe {
            let os_ctx = CONTEXT_OS.lock().as_mut_ptr();
            let (app_rsp, app_rip, app_ctx_ptr) = {
                // Release the lock of CONTEXT_APP before entering the app to make it available
                // from syscall handlers.
                let mut app_ctx = CONTEXT_APP.lock();
                (app_ctx.cpu.rsp, app_ctx.cpu.rip, app_ctx.as_mut_ptr())
            };
            asm!(
                // Save current execution state into CONTEXT_OS(rsi)
                // General registers
                "xchg rsp,rsi", // swap rsi with rsp to utilize push/pop
                "push rsi", // ExecutionContext.rsp
                "push r15",
                "push r14",
                "push r13",
                "push r12",
                "push r11",
                "push r10",
                "push r9",
                "push r8",
                "push rdi",
                "push rsi",
                "push rbp",
                "push rbx",
                "push rdx",
                "push rcx",
                "push rax",
                "pushfq", // ExecutionContext.rflags
                "lea r8, [rip+0f]", // ExecutionContext.rip, return address from app
                "push r8", // ExecutionContext.rip
                "sub rsp, 512",
                "fxsave64[rsp]",
                "xchg rsp,rsi", // recover the original rsp
                // At this point, the current CPU state is saved to CONTEXT_OS

                // Set data segments to USER_DS
                // rdx is passed from the Rust code (see the last part of this asm block).
                "mov es, dx",
                "mov ds, dx",
                "mov fs, dx",
                "mov gs, dx",

                // Prepare the stack to use iretq to switch to user mode
                "mov rsp, rax", // RSP = stack for app
                // Push values needed by `return_to_app`
                "sub rsp,64",
                "push rsp",
                "push r15",
                "push r14",
                "push r13",
                "push r12",
                "push r11",
                "push r10",
                "push r9",
                "push r8",
                "push rdi",
                "push rsi",
                "push rbp",
                "push rbx",
                "push rdx",
                "push rcx",
                "push rax",
                "push 2",     // RFLAGS saved on syscall
                "push rdi",     // RIP saved on syscall
                "sub rsp, 512", // FpuContext
                "fxsave64[rsp]",
                // Start (or resume) the app execution
                ".global return_to_app", // external symbol
                "jmp return_to_app",

                // ---- no one will pass through here ----

                // **** return from app via return_to_os() ****
                // See also: os/src/syscall.rs
                "0:",
                // At this point:
                // - context: CONTEXT_APP + handling syscall
                //   - so it's in the kernel mode
                // - rdi: addr of CONTEXT_OS
                // - rsi: addr of CONTEXT_APP

                // Recover the segment registers to OS
                "push rdi", // Use rdi as TMP
                "mov di,ss", // SS is already pointing the OS data segment
                             // (which is done by the CPU when entering syscall)
                             // so copy it to other segment registers.
                "mov ds,di",
                "mov es,di",
                "mov fs,di",
                "mov gs,di",
                "pop rdi",  // Recover rdi value

                // Save the cpu state to CONTEXT_APP

                // Load the cpu state from CONTEXT_OS
                "xchg rsp, rdi", // swap rsp and rdi to utilize push / pop
                "fxrstor64[rsp]",
                "add rsp, 512",
                "pop rax", // Skip RIP
                "popfq", // Restore RFLAGS
                "pop rax",
                "pop rcx",
                "pop rdx",
                "pop rbx",
                "pop rbp",
                "pop rsi",
                "pop rdi",
                "pop r8",
                "pop r9",
                "pop r10",
                "pop r11",
                "pop r12",
                "pop r13",
                "pop r14",
                "pop r15",
                "pop rsp",
                // At this point, the CPU state is same as the CONTEXT_OS except for RIP.
                // Returning to the Rust code and continue the execution.

                in("rax") app_rsp,
                in("rcx") crate::x86_64::USER64_CS,
                in("rdx") crate::x86_64::USER_DS,
                // rbx is used for LLVM internally
                in("rsi") (os_ctx as *mut u8).add(size_of::<ExecutionContext>()),
                in("r9") (app_ctx_ptr as *mut u8).add(size_of::<ExecutionContext>()),
                in("rdi") app_rip,
                lateout("rax") retcode,
                lateout("r8") exit_reason,
            );
        }
        if exit_reason == 0 {
            // return to os
            break;
        }
        yield_execution().await;
    }
    Ok(retcode)
}

// https://doc.rust-lang.org/reference/items/external-blocks.html#abi
// https://doc.rust-lang.org/reference/inline-assembly.html
// https://www.intel.com/content/dam/develop/public/us/en/documents/325383-sdm-vol-2abcd.pdf#page=1401
global_asm!(
    // **** Symbols from Rust code
    ".global arch_syscall_handler",
    // **** Implementations
    ".global asm_syscall_handler",
    "asm_syscall_handler:",
    // On syscall entry,
    // RCX <= RIP to continue execution
    // RIP <= IA32_LSTAR
    // R11 <= RFLAGS
    // RFLAGS &= IA32_FMASK
    // CS <= {
    //      Selector: IA32_STAR[32..48] & 0xFFFC
    //      Base: 0
    //      Attr: 64-bit code, accessed
    //      DPL: 0
    // }
    // SS <= {
    //      Selector: IA32_STAR[32..48] + 8
    //      Base: 0
    //      Attr: RW data, accessed
    //      DPL: 0
    // }

    // Preserve registers after syscall
    "sub rsp,64",
    "push rsp",
    "push r15",
    "push r14",
    "push r13",
    "push r12",
    "push 0", // r11 (destroyed)
    "push r10",
    "push r9",
    "push r8",
    "push rdi",
    "push rsi",
    "push rbp",
    "push rbx",
    "push rdx",
    "push 0", // rcx (destroyed)
    "push rax",
    "push r11",     // RFLAGS saved on syscall
    "push rcx",     // RIP saved on syscall
    "sub rsp, 512", // FpuContext
    "fxsave64[rsp]",
    //
    "mov rbp, rsp", // Save rsp to restore later
    "mov rdi, rsp", // First argument for syscall_handler (regs)
    "and rsp, -16", // Align the stack (to satisfy sysv64 ABI)
    "call arch_syscall_handler",
    "mov rsp, rbp", // Recover original stack value
    //
    ".global return_to_app",
    "return_to_app:",
    // Restore registers to sysret
    // This block assumes:
    // - RSP = User stack, with saved registers
    "fxrstor64[rsp]",
    "add rsp, 512", // FpuContext
    "pop rcx",      // RIP saved on syscall
    "pop r11",      // RFLAGS saved on syscall
    "pop rax",
    "add rsp, 8", // rcx (destroyed)
    "pop rdx",
    "pop rbx",
    "pop rbp",
    "pop rsi",
    "pop rdi",
    "pop r8",
    "pop r9",
    "pop r10",
    "add rsp, 8", // r11 (destroyed)
    "pop r12",
    "pop r13",
    "pop r14",
    "pop r15",
    "add rsp,8", // rsp (skip)
    "add rsp,64",
    //
    "sysretq",
    // sysretq will do:
    // RIP <= RCX
    // RFLAGS <=  (R11 & 0x3C7FD7) | 2      // Clear RF, VM, reserved bits; set bit 1
    // CS <= {
    //      Selector: (IA32_STAR[48..64] + 16) | 3      // RPL = 3
    //      Base: 0
    //      Attr: 64-bit code, accessed
    //      DPL: 3
    // }
    // SS <= {
    //      Selector: (IA32_STAR[48..64] + 8) | 3       // RPL = 3
    //      Base: 0
    //      Attr: RW data, accessed
    //      DPL: 3
    // }
);
