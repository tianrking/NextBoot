//! Checked geometry planning only; this module never writes a disk.

#[derive(Debug, PartialEq, Eq)]
pub struct GrowthPlan {
    pub volume_blocks: u64,
    pub cluster_count: u32,
}

pub fn plan_growth(
    old_blocks: u64,
    available_blocks: u64,
    heap_offset: u64,
    sectors_per_cluster: u64,
    fat_capacity: u64,
) -> Result<GrowthPlan, &'static str> {
    if available_blocks < old_blocks {
        return Err("refusing to shrink existing exFAT volume");
    }
    if !sectors_per_cluster.is_power_of_two() || old_blocks <= heap_offset {
        return Err("invalid exFAT growth geometry");
    }
    let capacity = fat_capacity.min(u64::from(u32::MAX - 2));
    if (old_blocks - heap_offset) / sectors_per_cluster > capacity {
        return Err("existing exFAT clusters exceed FAT capacity");
    }
    let capacity_blocks = capacity
        .checked_mul(sectors_per_cluster)
        .and_then(|blocks| blocks.checked_add(heap_offset))
        .ok_or("exFAT growth overflows")?;
    // VolumeLength includes trailing sectors that do not form a full cluster.
    // Preserve them on no-op runs instead of rounding the partition down.
    let volume_blocks = available_blocks.min(capacity_blocks.max(old_blocks));
    let cluster_count = (volume_blocks - heap_offset) / sectors_per_cluster;
    if cluster_count < 16 {
        return Err("expanded exFAT cluster count is too small");
    }
    Ok(GrowthPlan {
        volume_blocks,
        cluster_count: cluster_count as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_partial_cluster_tail_on_unchanged_media() {
        assert_eq!(
            plan_growth(1003, 1003, 100, 8, 1000),
            Ok(GrowthPlan {
                volume_blocks: 1003,
                cluster_count: 112
            })
        );
    }

    #[test]
    fn growth_uses_available_sectors_and_caps_clusters() {
        assert_eq!(
            plan_growth(1003, 2003, 100, 8, 1000).unwrap().volume_blocks,
            2003
        );
        assert_eq!(
            plan_growth(1003, 20003, 100, 8, 1000),
            Ok(GrowthPlan {
                volume_blocks: 8100,
                cluster_count: 1000
            })
        );
    }

    #[test]
    fn rejects_shrinking_and_invalid_capacity() {
        assert!(plan_growth(1003, 900, 100, 8, 1000).is_err());
        assert!(plan_growth(1003, 2000, 100, 8, 100).is_err());
        assert!(plan_growth(1003, 2000, 100, 0, 1000).is_err());
        assert!(plan_growth(1003, 2000, 100, 3, 1000).is_err());
    }
}
