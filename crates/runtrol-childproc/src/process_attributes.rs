//! One owning Windows process-attribute list for contained births.

use std::marker::PhantomData;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::HPCON;
use windows_sys::Win32::System::Threading::{
    DeleteProcThreadAttributeList, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, UpdateProcThreadAttribute,
};

use crate::SpawnError;
use crate::contain::job::last_error;

pub(crate) struct ProcessAttributes<'a> {
    buffer: Vec<usize>,
    _values: PhantomData<&'a HANDLE>,
}

impl<'a> ProcessAttributes<'a> {
    #[expect(
        unsafe_code,
        reason = "the explicit inherited HANDLE slice stays borrowed through CreateProcess"
    )]
    pub(crate) fn for_handles(handles: &'a [HANDLE]) -> Result<Self, SpawnError> {
        let mut attributes = Self::new(1)?;
        // SAFETY: the caller supplies only real inheritable handles and retains the slice for this list.
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.as_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error("selecting private inherited handles"));
        }
        Ok(attributes)
    }
    #[expect(
        unsafe_code,
        reason = "the Windows attribute list owns an aligned ABI allocation"
    )]
    fn new(count: u32) -> Result<Self, SpawnError> {
        let mut bytes = 0;
        // SAFETY: the documented sizing call has no list and writes only the required allocation size.
        unsafe {
            InitializeProcThreadAttributeList(core::ptr::null_mut(), count, 0, &raw mut bytes)
        };
        if bytes == 0 {
            return Err(last_error("sizing child process attributes"));
        }
        let mut result = Self {
            buffer: vec![0; bytes.div_ceil(size_of::<usize>())],
            _values: PhantomData,
        };
        // SAFETY: the initialized list fits its aligned backing allocation, retained until Drop.
        if unsafe { InitializeProcThreadAttributeList(result.as_ptr(), count, 0, &raw mut bytes) }
            == 0
        {
            result.buffer.clear();
            return Err(last_error("initializing child process attributes"));
        }
        Ok(result)
    }

    #[expect(
        unsafe_code,
        reason = "the pseudo-console value and borrowed Job use their documented attribute ABI"
    )]
    pub(crate) fn for_console(console: HPCON, job: &'a HANDLE) -> Result<Self, SpawnError> {
        let mut attributes = Self::new(2)?;
        // SAFETY: Windows defines this attribute's value as the console handle itself, rather than
        // a pointer to it. The console owner outlives process creation and this list.
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.as_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                core::ptr::with_exposed_provenance::<core::ffi::c_void>(console.cast_unsigned()),
                size_of::<HPCON>(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error(
                "attaching the pseudo console to process creation",
            ));
        }
        // SAFETY: the lifetime preserves this exact Job handle value through deletion of the list.
        if unsafe {
            UpdateProcThreadAttribute(
                attributes.as_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                core::ptr::from_ref(job).cast(),
                size_of::<HANDLE>(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_error("attaching the terminal Job to process creation"));
        }
        Ok(attributes)
    }

    pub(crate) fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
}

impl Drop for ProcessAttributes<'_> {
    #[expect(
        unsafe_code,
        reason = "the initialized attribute list must be deleted before its allocation"
    )]
    fn drop(&mut self) {
        if !self.buffer.is_empty() {
            // SAFETY: the list was initialized once and its aligned allocation is still owned here.
            unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
        }
    }
}
