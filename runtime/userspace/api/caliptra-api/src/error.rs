// Licensed under the Apache-2.0 license

use caliptra_mcu_libsyscall_caliptra::mailbox::MailboxError;
use caliptra_mcu_libtock_platform::ErrorCode;
use caliptra_ocp_eat::EatError;

pub type CaliptraApiResult<T> = Result<T, CaliptraApiError>;

#[derive(PartialEq)]
pub enum CaliptraApiError {
    MailboxBusy,
    Mailbox(MailboxError),
    Syscall(ErrorCode),
    InvalidArgument(&'static str),
    InvalidOperation(&'static str),
    AesGcmInvalidDataLength,
    AesGcmInvalidAadLength,
    AesGcmInvalidOperation,
    AesGcmInvalidContext,
    AesGcmTagVerifyFailed,
    AsymAlgoUnsupported,
    InvalidResponse,
    BufferTooSmall,
    UnprovisionedCsr,
    Eat(EatError),
}
impl core::fmt::Debug for CaliptraApiError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("CaliptraApiError")
    }
}
