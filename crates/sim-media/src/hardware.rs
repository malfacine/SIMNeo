use ffmpeg_next::{codec, ffi};
use std::ptr;

unsafe extern "C" fn choose_format(
    _context: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    // Lista terminada em NONE mantida pelo FFmpeg durante o callback.
    unsafe {
        let mut next = formats;
        let mut software = ffi::AVPixelFormat::AV_PIX_FMT_NONE;
        while *next != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *next == ffi::AVPixelFormat::AV_PIX_FMT_D3D11 {
                return *next;
            }
            let desc = ffi::av_pix_fmt_desc_get(*next);
            if software == ffi::AVPixelFormat::AV_PIX_FMT_NONE
                && !desc.is_null()
                && (*desc).flags & ffi::AV_PIX_FMT_FLAG_HWACCEL as u64 == 0
            {
                software = *next;
            }
            next = next.add(1);
        }
        software
    }
}

pub fn attach(context: &mut codec::context::Context) -> bool {
    // A referência criada pertence ao AVCodecContext, que a libera no Drop.
    unsafe {
        let codec = ffi::avcodec_find_decoder(context.id().into());
        if codec.is_null() {
            return false;
        }
        let mut supported = false;
        for index in 0..64 {
            let config = ffi::avcodec_get_hw_config(codec, index);
            if config.is_null() {
                break;
            }
            if (*config).device_type == ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
                && (*config).methods & ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32 != 0
            {
                supported = true;
                break;
            }
        }
        if !supported {
            return false;
        }
        let mut device = ptr::null_mut();
        if ffi::av_hwdevice_ctx_create(
            &mut device,
            ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            ptr::null(),
            ptr::null_mut(),
            0,
        ) < 0
        {
            ffi::av_buffer_unref(&mut device);
            return false;
        }
        let raw = context.as_mut_ptr();
        (*raw).hw_device_ctx = device;
        (*raw).get_format = Some(choose_format);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picks_hardware_when_offered_and_software_when_unavailable() {
        use ffi::AVPixelFormat::*;
        let hardware = [AV_PIX_FMT_D3D11, AV_PIX_FMT_YUV420P, AV_PIX_FMT_NONE];
        let software = [AV_PIX_FMT_DXVA2_VLD, AV_PIX_FMT_YUV420P, AV_PIX_FMT_NONE];
        assert_eq!(
            unsafe { choose_format(ptr::null_mut(), hardware.as_ptr()) },
            AV_PIX_FMT_D3D11
        );
        assert_eq!(
            unsafe { choose_format(ptr::null_mut(), software.as_ptr()) },
            AV_PIX_FMT_YUV420P
        );
    }
}
