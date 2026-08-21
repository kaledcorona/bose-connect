//! What this device answers.
//!
//! Every implementation of this protocol so far has answered that with a table
//! of device ids maintained by hand, and been wrong about every model its
//! author did not own. This asks the device.
//!
//! One enumeration per function in the catalog, at open. That is the cheap half
//! — `<fn> 01 05 00` returns a function's whole populated surface in a single
//! exchange, where probing opcode by opcode costs a round trip each and a
//! silent one costs the whole receive timeout.
//!
//! The expensive half is left undone until something asks: a function that
//! refuses to list itself may still be full of data, so its addresses stay
//! unproven and the first read settles them.

use std::collections::{BTreeMap, BTreeSet};

use crate::catalog;
use crate::error::Result;
use crate::session::{Listing, Session};
use crate::transport::Transport;
use crate::wire::{Addr, Refusal};

/// What is known about one address, before asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Known {
    /// Listed by an enumeration, or read successfully.
    Live,
    /// Its function is missing, its enumeration did not mention it, or a probe
    /// refused it.
    Absent,
    /// Its function will not list itself. Only a read will say.
    Unproven,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Surface {
    /// Functions that enumerated, with the opcodes they listed.
    ///
    /// Treated as authoritative for that function: an address it did not
    /// mention is reported absent. The reference's caveat — that an opcode
    /// holding only zeros appears to be omitted — means this can be wrong in
    /// one direction, so `scan` and the `raw` command bypass it.
    listed: BTreeMap<u8, BTreeSet<u8>>,
    /// Functions that answered at all, including those that refuse to list.
    functions: BTreeSet<u8>,
    /// Settled by a read rather than by a listing.
    probed: BTreeMap<Addr, bool>,
}

impl Surface {
    /// One enumeration per function the catalog mentions.
    pub fn discover<T: Transport>(session: &mut Session<T>) -> Result<Self> {
        let mut s = Surface::default();
        for f in catalog::functions() {
            match session.list(f)? {
                Listing::Records(ops) if !ops.is_empty() => {
                    s.functions.insert(f);
                    s.listed.insert(f, ops.into_iter().collect());
                }
                // Only `03` says the function is missing. Every other refusal
                // means it is there and will not say what it holds — five such
                // functions on the Ultra are full of data — so its addresses
                // stay unproven rather than being written off.
                Listing::Refused(Refusal::FunctionAbsent) | Listing::Silent => {}
                Listing::Records(_) | Listing::Refused(_) => {
                    s.functions.insert(f);
                }
            }
        }
        Ok(s)
    }

    pub fn state(&self, addr: Addr) -> Known {
        if let Some(&live) = self.probed.get(&addr) {
            return if live { Known::Live } else { Known::Absent };
        }
        match self.listed.get(&addr.function) {
            // Opcode 00 is every function's version. It is never listed and it
            // always answers, so the sweep will not hand it to you.
            Some(_) if addr.opcode == 0x00 => Known::Live,
            Some(ops) if ops.contains(&addr.opcode) => Known::Live,
            Some(_) => Known::Absent,
            None if self.functions.contains(&addr.function) => Known::Unproven,
            None => Known::Absent,
        }
    }

    /// Record what a read found. Only structural answers get here; see
    /// [`crate::device::Device::get`].
    pub fn settle(&mut self, addr: Addr, live: bool) {
        self.probed.insert(addr, live);
    }

    pub fn functions(&self) -> impl Iterator<Item = u8> + '_ {
        self.functions.iter().copied()
    }

    /// Every address known to hold a value, listed or proven.
    pub fn live(&self) -> impl Iterator<Item = Addr> + '_ {
        let listed = self
            .listed
            .iter()
            .flat_map(|(&f, ops)| ops.iter().map(move |&o| Addr::at(f, o)));
        let probed = self.probed.iter().filter(|&(_, &v)| v).map(|(&a, _)| a);
        listed.chain(probed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(listed: &[(u8, &[u8])], functions: &[u8]) -> Surface {
        Surface {
            listed: listed.iter().map(|(f, o)| (*f, o.iter().copied().collect())).collect(),
            functions: listed.iter().map(|(f, _)| *f).chain(functions.iter().copied()).collect(),
            probed: BTreeMap::new(),
        }
    }

    #[test]
    fn a_function_that_enumerated_answers_for_every_opcode_in_it() {
        // A QC35's function 01 lists 02 03 04 06. There is no 07, so there is
        // no equaliser — and saying so costs no round trip and no timeout,
        // which is the whole point of enumerating first.
        let s = surface(&[(0x01, &[0x02, 0x03, 0x04, 0x06])], &[]);
        assert_eq!(s.state(Addr::at(0x01, 0x06)), Known::Live);
        assert_eq!(s.state(Addr::at(0x01, 0x07)), Known::Absent);
    }

    #[test]
    fn opcode_00_is_live_although_no_enumeration_ever_lists_it() {
        let s = surface(&[(0x01, &[0x02])], &[]);
        assert_eq!(s.state(Addr::at(0x01, 0x00)), Known::Live);
    }

    #[test]
    fn a_function_that_refuses_to_list_leaves_its_opcodes_unproven() {
        // Five functions on the Ultra refuse the sweep and are full of data.
        // Writing them off is how immersive audio stayed hidden for eight
        // labelled observations.
        let s = surface(&[], &[0x05]);
        assert_eq!(s.state(Addr::at(0x05, 0x0f)), Known::Unproven);
        // A function that never answered at all is a different case.
        assert_eq!(s.state(Addr::at(0x1f, 0x03)), Known::Absent);
    }

    #[test]
    fn a_probe_overrides_a_listing() {
        let mut s = surface(&[(0x01, &[0x02])], &[]);
        assert_eq!(s.state(Addr::at(0x01, 0x0a)), Known::Absent);
        s.settle(Addr::at(0x01, 0x0a), true);
        assert_eq!(s.state(Addr::at(0x01, 0x0a)), Known::Live);
    }
}
