#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffinityError {
    InvalidCoreId,
    UnsupportedPlatform,
}

pub struct CpuAffinity;

impl CpuAffinity {
    pub fn pin_current_thread(core_id: usize) -> Result<(), AffinityError> {
        if core_id == usize::MAX {
            return Err(AffinityError::InvalidCoreId);
        }
        #[cfg(target_os = "linux")]
        {
            let core_ids =
                core_affinity::get_core_ids().ok_or(AffinityError::UnsupportedPlatform)?;
            let core = core_ids
                .into_iter()
                .find(|core| core.id == core_id)
                .ok_or(AffinityError::InvalidCoreId)?;
            core_affinity::set_for_current(core);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = core_id;
            Ok(())
        }
    }
}
