//! What the game crashed on, in its log.
//!
//! A log that ends without "The game is closing" ended in a crash or a kill, and nothing said
//! which, or where (a game Steam or Ubisoft Connect starts doesn't even give the launcher its
//! exit code). A vectored exception handler writes the first few serious exceptions (an access
//! violation, an illegal instruction, a division by zero, a stack overflow) to the log: the code,
//! where it happened (module and offset) and the code addresses found on the stack. It's called
//! before the game's own handlers, so an exception the game goes on to handle is written too,
//! said as much, and at most [`MAX_WRITTEN`] a run.
//!
//! A crashing game may hold the heap's lock or the loader lock, and tracing may be what crashed,
//! so the handler allocates nothing and waits on nothing: it formats into a buffer on its stack
//! and appends to the file with `CreateFileW`/`WriteFile`, the file's name worked out at start.
//! Modules are named from their own export directory in memory, found with `VirtualQuery`.

use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

/// The most exceptions written in one run.
const MAX_WRITTEN: u32 = 3;
/// The most code addresses listed from the stack, and how far up it they're looked for.
const MAX_FRAMES: usize = 10;
const STACK_WORDS: usize = 1024;

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const ACCESS_VIOLATION: u32 = 0xC000_0005;
const STACK_OVERFLOW: u32 = 0xC000_00FD;

const MEM_COMMIT: u32 = 0x1000;
const MEM_IMAGE: u32 = 0x0100_0000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_GUARD: u32 = 0x100;
const PAGE_EXECUTE_ANY: u32 = 0x10 | 0x20 | 0x40 | 0x80;

const FILE_APPEND_DATA: u32 = 0x4;
const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE: isize = -1;

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut c_void,
    parameters: u32,
    information: [usize; 15],
}

#[repr(C)]
struct ExceptionPointers {
    record: *mut ExceptionRecord,
    /// The x86 `CONTEXT`; only `Eip` and `Esp` are read, by their offsets.
    context: *const u8,
}

const CONTEXT_EIP: usize = 0xB8;
const CONTEXT_ESP: usize = 0xC4;

#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    base: usize,
    allocation_base: usize,
    allocation_protect: u32,
    size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Default)]
struct SystemTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: unsafe extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;
    fn VirtualQuery(address: *const c_void, info: *mut MemoryInfo, length: usize) -> usize;
    fn CreateFileW(name: *const u16, access: u32, share: u32, security: *const c_void, disposition: u32, flags: u32, template: isize) -> isize;
    fn WriteFile(file: isize, buffer: *const u8, length: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetSystemTime(time: *mut SystemTime);
}

/// The log's name, wide and nul-terminated.
static LOG: OnceLock<Vec<u16>> = OnceLock::new();
/// The game's executable: where it's loaded, and its file name (an executable has no export
/// directory to name it).
static EXE_BASE: AtomicUsize = AtomicUsize::new(0);
static EXE_NAME: OnceLock<String> = OnceLock::new();
static WRITTEN: AtomicU32 = AtomicU32::new(0);

/// Starts writing the game's serious exceptions to `log`. Once, at start.
pub fn install(log: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt as _;
    if LOG.set(log.as_os_str().encode_wide().chain(Some(0)).collect()).is_err() {
        return;
    }
    if let Some((base, _)) = unsafe { crate::module_bounds() } {
        EXE_BASE.store(base, Ordering::Relaxed);
    }
    if let Some(name) = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())) {
        let _ = EXE_NAME.set(name);
    }
    // Last in the list of vectored handlers (any the game adds go first), but before the
    // game's own __try/__except: an exception it handles is still seen here.
    if unsafe { AddVectoredExceptionHandler(0, handler) }.is_null() {
        tracing::warn!("Crash details: the exception handler couldn't be added");
    }
}

fn what(code: u32) -> Option<&'static str> {
    Some(match code {
        ACCESS_VIOLATION => "access violation",
        0xC000_001D => "illegal instruction",
        0xC000_0096 => "privileged instruction",
        0xC000_0094 => "integer division by zero",
        0xC000_0095 => "integer overflow",
        STACK_OVERFLOW => "stack overflow",
        0xC000_0006 => "in-page error",
        0xC000_0374 => "heap corruption",
        0xC000_008C => "array bounds exceeded",
        _ => return None,
    })
}

unsafe extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    let Some(pointers) = pointers.as_ref() else { return EXCEPTION_CONTINUE_SEARCH };
    let Some(record) = pointers.record.as_ref() else { return EXCEPTION_CONTINUE_SEARCH };
    let Some(name) = what(record.code) else { return EXCEPTION_CONTINUE_SEARCH };
    if WRITTEN.fetch_add(1, Ordering::Relaxed) >= MAX_WRITTEN {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let mut line = Line::new();
    timestamp(&mut line);
    let _ = write!(line, " ERROR hooks::crash: The game hit an exception: {:#010X} {name}", record.code);
    if record.code == ACCESS_VIOLATION && record.parameters >= 2 {
        let how = match record.information[0] {
            0 => "reading",
            1 => "writing",
            _ => "executing",
        };
        let _ = write!(line, " ({how} {:#010x})", record.information[1]);
    }
    line.push(" at ");
    place(&mut line, record.address as usize);
    // A stack overflow leaves this handler little stack: no walk then.
    if record.code != STACK_OVERFLOW && !pointers.context.is_null() {
        let esp = std::ptr::read_unaligned(pointers.context.add(CONTEXT_ESP).cast::<u32>()) as usize;
        let eip = std::ptr::read_unaligned(pointers.context.add(CONTEXT_EIP).cast::<u32>()) as usize;
        if eip != record.address as usize {
            line.push(" (eip ");
            place(&mut line, eip);
            line.push(")");
        }
        line.push(". On the stack:");
        stack(&mut line, esp);
    }
    line.push(". If the game handles it, it carries on.\n");
    append(line.bytes());
    EXCEPTION_CONTINUE_SEARCH
}

/// `module+0xoffset`, or the bare address outside any module.
unsafe fn place(line: &mut Line, address: usize) {
    match module_of(address) {
        Some(base) => {
            module_name(line, base);
            let _ = write!(line, "+{:#x}", address - base);
        }
        None => {
            let _ = write!(line, "{address:#010x}");
        }
    }
}

/// The code addresses found in the stack's words above `esp`: return addresses, mostly.
unsafe fn stack(line: &mut Line, esp: usize) {
    let mut found = 0;
    let mut page_end = 0;
    for i in 0..STACK_WORDS {
        let at = esp + i * 4;
        if at + 4 > page_end {
            let Some(end) = readable_until(at) else { break };
            page_end = end;
        }
        let value = std::ptr::read_unaligned(at as *const u32) as usize;
        if is_code(value) {
            line.push(if found == 0 { " " } else { ", " });
            place(line, value);
            found += 1;
            if found == MAX_FRAMES {
                break;
            }
        }
    }
    if found == 0 {
        line.push(" nothing readable");
    }
}

unsafe fn query(address: usize) -> Option<MemoryInfo> {
    let mut info = MemoryInfo::default();
    (VirtualQuery(address as *const c_void, &mut info, std::mem::size_of::<MemoryInfo>()) != 0).then_some(info)
}

/// Where the readable memory that `address` is in ends.
unsafe fn readable_until(address: usize) -> Option<usize> {
    let info = query(address)?;
    (info.state == MEM_COMMIT && info.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && info.protect != 0).then(|| info.base + info.size)
}

unsafe fn readable(address: usize, length: usize) -> bool {
    readable_until(address).is_some_and(|end| address.checked_add(length).is_some_and(|last| last <= end))
}

unsafe fn is_code(address: usize) -> bool {
    address > 0x10000 && query(address).is_some_and(|i| i.state == MEM_COMMIT && i.kind == MEM_IMAGE && i.protect & PAGE_EXECUTE_ANY != 0)
}

/// The base of the module (`.exe` or `.dll`) that `address` is in.
unsafe fn module_of(address: usize) -> Option<usize> {
    query(address).filter(|i| i.kind == MEM_IMAGE && i.allocation_base != 0).map(|i| i.allocation_base)
}

/// The module's name: the executable's file name, or the name in a DLL's export directory.
unsafe fn module_name(line: &mut Line, base: usize) {
    if base == EXE_BASE.load(Ordering::Relaxed) {
        if let Some(name) = EXE_NAME.get() {
            line.push(name);
            return;
        }
    }
    if let Some(name) = export_name(base) {
        line.push_bytes(name);
    } else {
        let _ = write!(line, "{base:#010x}");
    }
}

/// The name in a PE32 image's export directory, read from memory.
unsafe fn export_name(base: usize) -> Option<&'static [u8]> {
    let read_u32 = |at: usize| readable(at, 4).then(|| std::ptr::read_unaligned(at as *const u32) as usize);
    let nt = base + read_u32(base + 0x3c)?;
    // The PE32 optional header's first data directory (exports) is 96 bytes into it.
    let exports = read_u32(nt + 24 + 96)?;
    if exports == 0 {
        return None;
    }
    let name = base + read_u32(base + exports + 12)?;
    let mut length = 0;
    while length < 64 && readable(name + length, 1) && *((name + length) as *const u8) != 0 {
        length += 1;
    }
    (length > 0).then(|| std::slice::from_raw_parts(name as *const u8, length))
}

/// The time as the log's other lines have it (`2026-10-16T19:43:27.038000Z`).
unsafe fn timestamp(line: &mut Line) {
    let mut t = SystemTime::default();
    GetSystemTime(&mut t);
    let _ = write!(
        line,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}000Z",
        t.year, t.month, t.day, t.hour, t.minute, t.second, t.milliseconds
    );
}

unsafe fn append(bytes: &[u8]) {
    let Some(name) = LOG.get() else { return };
    let file = CreateFileW(name.as_ptr(), FILE_APPEND_DATA, FILE_SHARE_READ_WRITE, std::ptr::null(), OPEN_EXISTING, 0, 0);
    if file == INVALID_HANDLE || file == 0 {
        return;
    }
    let mut written = 0;
    WriteFile(file, bytes.as_ptr(), bytes.len() as u32, &mut written, std::ptr::null_mut());
    CloseHandle(file);
}

/// A line built on the stack: what doesn't fit is cut.
struct Line {
    buffer: [u8; 1024],
    length: usize,
}

impl Line {
    fn new() -> Self {
        Self { buffer: [0; 1024], length: 0 }
    }

    fn push(&mut self, text: &str) {
        self.push_bytes(text.as_bytes());
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        // Keep room for the line's end.
        let room = self.buffer.len() - 1 - self.length;
        let n = bytes.len().min(room);
        self.buffer[self.length..self.length + n].copy_from_slice(&bytes[..n]);
        self.length += n;
    }

    fn bytes(&mut self) -> &[u8] {
        if self.buffer[..self.length].last() != Some(&b'\n') {
            self.buffer[self.length] = b'\n';
            self.length += 1;
        }
        &self.buffer[..self.length]
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push(s);
        Ok(())
    }
}
