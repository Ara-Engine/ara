//! Elevated foreground graphics scheduling is optional, including OS permission.
use ash::vk;

pub(super) fn create<T>(
    request_high: bool,
    mut create: impl FnMut(bool) -> Result<T, vk::Result>,
) -> Result<(T, bool), vk::Result> {
    match create(request_high) {
        Ok(device) => Ok((device, request_high)),
        Err(vk::Result::ERROR_NOT_PERMITTED | vk::Result::ERROR_INITIALIZATION_FAILED)
            if request_high =>
        {
            create(false).map(|device| (device, false))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_or_unavailable_high_priority_retries_once_at_normal_priority() {
        for denied in [
            vk::Result::ERROR_NOT_PERMITTED,
            vk::Result::ERROR_INITIALIZATION_FAILED,
        ] {
            let mut attempts = Vec::new();
            let result = create(true, |high| {
                attempts.push(high);
                if high { Err(denied) } else { Ok(7) }
            });
            assert_eq!(result, Ok((7, false)));
            assert_eq!(attempts, [true, false]);
        }
        let mut attempts = Vec::new();
        assert_eq!(
            create(false, |high| {
                attempts.push(high);
                Err::<(), _>(vk::Result::ERROR_INITIALIZATION_FAILED)
            }),
            Err(vk::Result::ERROR_INITIALIZATION_FAILED)
        );
        assert_eq!(attempts, [false]);
    }

    #[test]
    fn accepted_high_and_unrelated_failures_do_not_retry() {
        assert_eq!(create(true, Ok), Ok((true, true)));
        let mut attempts = 0;
        assert_eq!(
            create(true, |_| {
                attempts += 1;
                Err::<(), _>(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)
            }),
            Err(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)
        );
        assert_eq!(attempts, 1);
    }
}
