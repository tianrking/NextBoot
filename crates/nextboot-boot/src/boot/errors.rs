use crate::wim;
use crate::xz::XzDecodeError;
use nextboot_fs::FsError;
use nextboot_virtio::VirtIoError;
use uefi::Status;

pub(super) fn virtio_error_to_uefi_status(err: VirtIoError) -> Status {
    match err {
        VirtIoError::OutOfBounds | VirtIoError::InvalidMapping => Status::LOAD_ERROR,
        VirtIoError::WriteProtected => Status::WRITE_PROTECTED,
        VirtIoError::InvalidArgument | VirtIoError::InvalidBufferSize => Status::INVALID_PARAMETER,
        VirtIoError::MediaChanged => Status::MEDIA_CHANGED,
        VirtIoError::NoPhysicalRead => Status::NO_MEDIA,
        VirtIoError::CrcError => Status::CRC_ERROR,
        VirtIoError::ReadFailed | VirtIoError::DeviceError => Status::DEVICE_ERROR,
    }
}

pub(super) fn virtio_error_to_fs_error(err: VirtIoError) -> FsError {
    match err {
        VirtIoError::InvalidArgument | VirtIoError::InvalidBufferSize => FsError::InvalidArgument,
        VirtIoError::OutOfBounds
        | VirtIoError::MediaChanged
        | VirtIoError::InvalidMapping
        | VirtIoError::NoPhysicalRead
        | VirtIoError::ReadFailed
        | VirtIoError::DeviceError
        | VirtIoError::CrcError => FsError::ReadError,
        VirtIoError::WriteProtected => FsError::UnsupportedFs,
    }
}

pub(super) fn fs_error_to_uefi_status(err: FsError) -> Status {
    match err {
        FsError::FileNotFound | FsError::DirectoryNotFound => Status::NOT_FOUND,
        FsError::InvalidPath | FsError::InvalidArgument => Status::INVALID_PARAMETER,
        FsError::OutOfMemory | FsError::FileTooLarge => Status::OUT_OF_RESOURCES,
        FsError::NotDirectory | FsError::NotFile | FsError::UnsupportedFs => Status::UNSUPPORTED,
        FsError::InvalidSignature | FsError::BlockSizeMismatch | FsError::Corrupted => {
            Status::LOAD_ERROR
        }
        FsError::ReadError => Status::DEVICE_ERROR,
    }
}

pub(super) fn xz_error_to_uefi_status(err: XzDecodeError) -> Status {
    match err {
        XzDecodeError::OutputTooLarge | XzDecodeError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
        XzDecodeError::Decoder(_) | XzDecodeError::Stalled => Status::LOAD_ERROR,
    }
}

pub(super) fn wim_read_error_to_uefi_status(err: wim::WimReadError) -> Status {
    match err {
        wim::WimReadError::InvalidChunkLength
        | wim::WimReadError::InvalidRange
        | wim::WimReadError::InvalidChunkTable
        | wim::WimReadError::XpressDecodeFailed(_)
        | wim::WimReadError::LzxDecodeFailed(_) => Status::LOAD_ERROR,
        wim::WimReadError::ResourceOutOfBounds => Status::DEVICE_ERROR,
        wim::WimReadError::OutputReserveFailed => Status::OUT_OF_RESOURCES,
        wim::WimReadError::UnsupportedCompressedChunk { .. } => Status::UNSUPPORTED,
    }
}

pub(super) fn ventoy_error_to_uefi_status(err: crate::ventoy::VentoyParamError) -> Status {
    match err {
        crate::ventoy::VentoyParamError::PathTooLong
        | crate::ventoy::VentoyParamError::InvalidSectorSize => Status::INVALID_PARAMETER,
        crate::ventoy::VentoyParamError::UnalignedExtent => Status::UNSUPPORTED,
        crate::ventoy::VentoyParamError::ValueOutOfRange
        | crate::ventoy::VentoyParamError::OutputTooLarge
        | crate::ventoy::VentoyParamError::OutputReserveFailed => Status::OUT_OF_RESOURCES,
    }
}

pub(super) fn ventoy_linux_error_to_uefi_status(
    err: crate::ventoy_linux::VentoyLinuxInitrdError,
) -> Status {
    match err {
        crate::ventoy_linux::VentoyLinuxInitrdError::InvalidArchive => Status::LOAD_ERROR,
        crate::ventoy_linux::VentoyLinuxInitrdError::InvalidSectorSize
        | crate::ventoy_linux::VentoyLinuxInitrdError::NameTooLong => Status::INVALID_PARAMETER,
        crate::ventoy_linux::VentoyLinuxInitrdError::UnalignedExtent => Status::UNSUPPORTED,
        crate::ventoy_linux::VentoyLinuxInitrdError::ValueOutOfRange
        | crate::ventoy_linux::VentoyLinuxInitrdError::FileTooLarge
        | crate::ventoy_linux::VentoyLinuxInitrdError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
    }
}

pub(super) fn ventoy_windows_runtime_data_error_to_uefi_status(
    err: nextboot_windows::VentoyWindowsRuntimeDataError,
) -> Status {
    match err {
        nextboot_windows::VentoyWindowsRuntimeDataError::AutoInstallTooLarge
        | nextboot_windows::VentoyWindowsRuntimeDataError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
    }
}

pub(super) fn ventoy_windows_wimboot_payload_error_to_uefi_status(
    err: nextboot_windows::VentoyWindowsWimbootPayloadError,
) -> Status {
    match err {
        nextboot_windows::VentoyWindowsWimbootPayloadError::OutputReserveFailed => {
            Status::OUT_OF_RESOURCES
        }
    }
}
