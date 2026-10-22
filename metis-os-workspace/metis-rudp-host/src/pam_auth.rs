//! Shared PAM password check for the lock screen and Metis Remote (RUDP).
//!
//! Hand-written libpam FFI (no `pam-sys` / bindgen). Never logs passwords.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

/// PAM service to authenticate against. Prefers `/etc/pam.d/metis`, then
/// common login stacks.
pub fn pam_service() -> String {
    for name in ["metis", "system-login", "login"] {
        if std::path::Path::new("/etc/pam.d").join(name).exists() {
            return name.to_string();
        }
    }
    "login".to_string()
}

const PAM_SUCCESS: c_int = 0;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
/// Reject empty auth tokens so `pam_unix` `nullok` cannot unlock without a
/// real password / biometric module success.
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x0001;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            num_msg: c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

enum PamHandle {}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        handle: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_authenticate(handle: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(handle: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_end(handle: *mut PamHandle, status: c_int) -> c_int;
}

struct ConvData {
    user: CString,
    password: CString,
}

unsafe extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    unsafe {
        if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
            return PAM_CONV_ERR;
        }
        let data = &*(appdata_ptr as *const ConvData);
        let n = num_msg as usize;
        let responses = libc::calloc(n, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if responses.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..n {
            let message = *msg.add(i);
            let out = responses.add(i);
            (*out).resp = std::ptr::null_mut();
            (*out).resp_retcode = 0;
            if message.is_null() {
                continue;
            }
            match (*message).msg_style {
                PAM_PROMPT_ECHO_OFF => {
                    (*out).resp = libc::strdup(data.password.as_ptr());
                }
                PAM_PROMPT_ECHO_ON => {
                    (*out).resp = libc::strdup(data.user.as_ptr());
                }
                PAM_TEXT_INFO | PAM_ERROR_MSG => {
                    (*out).resp = libc::strdup(c"".as_ptr());
                }
                _ => {}
            }
        }
        *resp = responses;
        PAM_SUCCESS
    }
}

/// Authenticate `user`/`password` against PAM `service`. Never logs the password.
pub fn pam_check(service: &str, user: &str, password: &str) -> bool {
    let (Ok(c_service), Ok(c_user_start)) = (CString::new(service), CString::new(user)) else {
        return false;
    };
    let (Ok(conv_user), Ok(conv_pass)) = (CString::new(user), CString::new(password)) else {
        return false;
    };
    let mut data = ConvData {
        user: conv_user,
        password: conv_pass,
    };
    let conv = PamConv {
        conv: Some(converse),
        appdata_ptr: &mut data as *mut ConvData as *mut c_void,
    };
    let mut handle: *mut PamHandle = std::ptr::null_mut();
    unsafe {
        let start = pam_start(
            c_service.as_ptr(),
            c_user_start.as_ptr(),
            &conv,
            &mut handle,
        );
        if start != PAM_SUCCESS || handle.is_null() {
            tracing::warn!(service, "pam_start failed");
            return false;
        }
        let auth = pam_authenticate(handle, PAM_DISALLOW_NULL_AUTHTOK);
        let acct = if auth == PAM_SUCCESS {
            pam_acct_mgmt(handle, PAM_DISALLOW_NULL_AUTHTOK)
        } else {
            auth
        };
        pam_end(handle, auth);
        auth == PAM_SUCCESS && acct == PAM_SUCCESS
    }
}

/// Resolve the current user's login name (env first, then passwd).
pub fn current_username() -> Option<String> {
    for var in ["USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if !pw.is_null() {
            let name = (*pw).pw_name;
            if !name.is_null()
                && let Ok(s) = CStr::from_ptr(name).to_str()
                && !s.is_empty()
            {
                return Some(s.to_string());
            }
        }
    }
    None
}
