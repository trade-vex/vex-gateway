pub mod auth_extractors;

pub use auth_extractors::AuthenticatedUserId;

use uuid::Uuid;

pub fn uuid_to_u64(uuid: Uuid) -> u64 {
    let raw = uuid.as_u128();
    ((raw >> 64) as u64) ^ (raw as u64)
}

#[cfg(test)]
mod tests {
    use super::uuid_to_u64;
    use uuid::Uuid;

    #[test]
    fn uuid_to_u64_xors_the_two_halves() {
        let uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);

        assert_eq!(uuid_to_u64(uuid), u64::MAX);
    }
}
