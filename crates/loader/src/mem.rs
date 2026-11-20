//! Page-level memory: reserve a region, set protections, free it on drop.

use std::io;

/// Page protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prot {
    Read,
    ReadWrite,
    ReadExecute,
    ReadWriteExecute,
}

/// A committed, page aligned region owned by this value.
#[derive(Debug)]
pub struct Region {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the region is plain memory owned by this value; access is
// synchronised by whoever holds it.
unsafe impl Send for Region {}
// SAFETY: as above; shared references only hand out the address.
unsafe impl Sync for Region {}

impl Region {
    pub fn addr(&self) -> usize {
        self.ptr as usize
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The whole region as bytes. Only valid while it is still writable.
    ///
    /// # Safety
    /// No other reference into the region may exist, and every page must be
    /// readable and writable.
    pub unsafe fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: the caller guarantees exclusive access and RW pages.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

pub fn page_size() -> usize {
    sys::page_size()
}

/// Reserves and commits `len` bytes, readable and writable, zero filled.
/// Tries `preferred` first when given, and takes any address otherwise.
pub fn alloc(len: usize, preferred: Option<usize>) -> io::Result<Region> {
    let len = len.div_ceil(page_size()) * page_size();
    if let Some(at) = preferred {
        if let Some(ptr) = sys::alloc_at(len, at) {
            return Ok(Region { ptr, len });
        }
    }
    let ptr = sys::alloc_any(len)?;
    Ok(Region { ptr, len })
}

/// Sets the protection of `len` bytes at `offset` inside the region.
pub fn protect(region: &Region, offset: usize, len: usize, prot: Prot) -> io::Result<()> {
    if len == 0 {
        return Ok(());
    }
    let page = page_size();
    assert_eq!(offset % page, 0, "protection must start on a page boundary");
    let len = len.div_ceil(page) * page;
    assert!(
        offset + len <= region.len,
        "protection past the end of the region"
    );
    // SAFETY: the range lies inside a region this process owns.
    unsafe { sys::protect(region.ptr.add(offset), len, prot) }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: the region was allocated by `alloc` and is freed once.
        unsafe { sys::free(self.ptr, self.len) }
    }
}

#[cfg(unix)]
mod sys {
    use super::Prot;
    use std::io;

    pub fn page_size() -> usize {
        // SAFETY: sysconf has no preconditions.
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    const RW: libc::c_int = libc::PROT_READ | libc::PROT_WRITE;
    const FLAGS: libc::c_int = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;

    pub fn alloc_at(len: usize, at: usize) -> Option<*mut u8> {
        // SAFETY: an anonymous mapping with a hint; the result is checked.
        let p = unsafe { libc::mmap(at as *mut _, len, RW, FLAGS, -1, 0) };
        if p == libc::MAP_FAILED {
            return None;
        }
        if p as usize != at {
            // SAFETY: unmapping the mapping that was just created.
            unsafe { libc::munmap(p, len) };
            return None;
        }
        Some(p.cast())
    }

    pub fn alloc_any(len: usize) -> io::Result<*mut u8> {
        // SAFETY: an anonymous mapping anywhere; the result is checked.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, RW, FLAGS, -1, 0) };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(p.cast())
    }

    /// # Safety
    /// `ptr..ptr+len` must be pages of a mapping this process owns.
    pub unsafe fn protect(ptr: *mut u8, len: usize, prot: Prot) -> io::Result<()> {
        let p = match prot {
            Prot::Read => libc::PROT_READ,
            Prot::ReadWrite => RW,
            Prot::ReadExecute => libc::PROT_READ | libc::PROT_EXEC,
            Prot::ReadWriteExecute => RW | libc::PROT_EXEC,
        };
        // SAFETY: guaranteed by the caller.
        if unsafe { libc::mprotect(ptr.cast(), len, p) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// # Safety
    /// `ptr` and `len` must describe one mapping made by this module.
    pub unsafe fn free(ptr: *mut u8, len: usize) {
        // SAFETY: guaranteed by the caller.
        unsafe { libc::munmap(ptr.cast(), len) };
    }
}

#[cfg(windows)]
mod sys {
    use super::Prot;
    use std::io;
    use windows_sys::Win32::System::Memory::*;
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

    pub fn page_size() -> usize {
        // SAFETY: GetSystemInfo fills the struct and cannot fail.
        unsafe {
            let mut info: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut info);
            info.dwPageSize as usize
        }
    }

    pub fn alloc_at(len: usize, at: usize) -> Option<*mut u8> {
        // SAFETY: VirtualAlloc at a requested address; the result is checked.
        let p = unsafe {
            VirtualAlloc(
                at as *const _,
                len,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if p.is_null() {
            None
        } else {
            Some(p.cast())
        }
    }

    pub fn alloc_any(len: usize) -> io::Result<*mut u8> {
        // SAFETY: VirtualAlloc anywhere; the result is checked.
        let p = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if p.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(p.cast())
    }

    /// # Safety
    /// `ptr..ptr+len` must be pages of an allocation this process owns.
    pub unsafe fn protect(ptr: *mut u8, len: usize, prot: Prot) -> io::Result<()> {
        let p = match prot {
            Prot::Read => PAGE_READONLY,
            Prot::ReadWrite => PAGE_READWRITE,
            Prot::ReadExecute => PAGE_EXECUTE_READ,
            Prot::ReadWriteExecute => PAGE_EXECUTE_READWRITE,
        };
        let mut old = 0;
        // SAFETY: guaranteed by the caller.
        if unsafe { VirtualProtect(ptr.cast(), len, p, &mut old) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// # Safety
    /// `ptr` must be the base of an allocation made by this module.
    pub unsafe fn free(ptr: *mut u8, _len: usize) {
        // SAFETY: guaranteed by the caller.
        unsafe { VirtualFree(ptr.cast(), 0, MEM_RELEASE) };
    }
}
