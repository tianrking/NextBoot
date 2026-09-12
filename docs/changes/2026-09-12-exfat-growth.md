# exFAT growth geometry planner

`nextboot_fs::exfat::growth::plan_growth` is a pure checked geometry API used by
the firmware growth path. It returns volume length and cluster count, preserves
partial-cluster tail sectors, rejects shrink requests, and caps growth at FAT
capacity. It does not perform I/O. Host tests cover the previously rejected
unchanged-media case and capped growth.
