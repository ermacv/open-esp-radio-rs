use PhyMode::{He20, Ht20, Ht40};

use Preference::{Automatic, ForceHt20, PreferHe20};

use super::*;

#[test]
fn preference_respects_admitted_modes() {
    for (ht40, he20, automatic, prefer_he20) in [
        (false, false, Ht20, Ht20),
        (true, false, Ht40, Ht40),
        (false, true, He20, He20),
        (true, true, Ht40, He20),
    ] {
        assert_eq!(select_phy(Automatic, ht40, he20), automatic);
        assert_eq!(select_phy(PreferHe20, ht40, he20), prefer_he20);
        assert_eq!(select_phy(ForceHt20, ht40, he20), Ht20);
    }
}
