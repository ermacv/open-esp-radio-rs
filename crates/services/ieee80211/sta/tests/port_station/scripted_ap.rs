//! A scripted access point for host tests of the station over the
//! lower-MAC host model.
//!
//! The access point reads every attempt the station submitted to the model
//! (`LowerMacModel::submitted`) and answers it by delivering frames through
//! `LowerMacModel::receive`: Probe Responses, Open System and SAE
//! Authentication, Association Responses, the WPA2 four-way handshake as
//! authenticator, and whatever data, Block Ack or management frames a test
//! queues. It sends beacons with a TIM at its TBTTs and reports each TBTT to
//! the station interface first. The model sends nothing on the air and
//! applies no cipher, so the access point reads the station's MPDUs in the
//! clear and marks the protected MPDUs it delivers as decrypted by the
//! backend.

use super::Model;
use std::{collections::VecDeque, vec, vec::Vec};

use oer_ieee80211_lower_mac::{
    Band, Channel, ChannelWidth, KeySelector, RxCryptoStatus, RxEvidence, RxMeta, VifId,
};
use oer_ieee80211_mac::{
    ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber},
    security::rsn::Akm,
};
use oer_ieee80211_rsn::{
    EapolKeyFrame, EapolKeyMessage, Pmk, Ptk, PtkContext,
    aes::software_aes128_key_wrap,
    frames::{OwnedRsnIe, RsnGtk, RsnIgtk, RsnPlainKeyData, RsnTxFrame},
    sae::{SAE_COMMIT_LEN, SaeCommit, SaeCommitValues, SaeKeys},
};

pub const AP: [u8; 6] = [0x02, 0xaa, 0, 0, 0, 1];
pub const STA: [u8; 6] = [0x02, 0x55, 0, 0, 0, 2];
pub const SSID: &[u8] = b"portnet";
pub const PASSPHRASE: &[u8] = b"correct horse battery";
pub const AP_CHANNEL: u8 = 6;
pub const RATES: [u8; 8] = [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24];
pub const ASSOCIATION_ID: u16 = 1;
pub const BEACON_INTERVAL_TU: u16 = 100;
pub const ANONCE: [u8; 32] = [0x44; 32];
pub const SNONCE: [u8; 32] = [0x33; 32];
pub const GTK: [u8; 16] = [0x5a; 16];
/// CCMP group, CCMP pairwise, PSK, no capabilities.
pub const RSN_PSK: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
/// CCMP group, CCMP pairwise, SAE, management frame protection required
/// and capable, BIP-CMAC-128.
pub const RSN_SAE: [u8; 28] = [
    0x30, 26, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 8, 0xc0, 0, 0,
    0, 0, 0x0f, 0xac, 6,
];
pub const IGTK: [u8; 16] = [0x6b; 16];

const EAPOL: u16 = 0x888e;

/// The security the access point runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSecurity {
    Open,
    Wpa2Psk,
    Sae,
}

/// One MSDU the station sent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Uplink {
    pub destination: [u8; 6],
    pub ether_type: u16,
    pub payload: Vec<u8>,
    pub qos_tid: Option<u8>,
    pub sequence: u16,
    pub protected: bool,
    pub key: KeySelector,
}

pub struct ScriptedAp {
    pub security: ApSecurity,
    pub seen: usize,
    pub outbox: VecDeque<(Vec<u8>, RxMeta)>,
    sequence: u16,
    pub probe_requests: Vec<u8>,
    /// The destination of every Probe Request.
    pub probe_destinations: Vec<[u8; 6]>,
    /// It answers Probe Requests.
    pub answers_probes: bool,
    pub authentications: usize,
    pub associations: usize,
    pub eapol_from_station: Vec<EapolKeyMessage>,
    pub handshake_complete: bool,
    pub uplink: Vec<Uplink>,
    /// The Power Management bit of each Null frame.
    pub nulls: Vec<bool>,
    pub actions: Vec<Vec<u8>>,
    pub deauthenticated: bool,
    /// Beacons from this time on, one per interval.
    pub next_beacon_micros: Option<u64>,
    /// The TIM of the next beacons names the station.
    pub tim_unicast: bool,
    pub beacons_sent: usize,
    pub tsf_offset: u64,
    sae_commit: Option<SaeCommit>,
    sae_keys: Option<SaeKeys>,
    pmk: Option<Pmk>,
    /// The keys of the completed four-way handshake.
    ptk: Option<Ptk>,
    /// The WMM Parameter Element of its beacons and Probe Responses.
    pub wmm_beacon: Option<Vec<u8>>,
    /// Its beacons and Probe Responses carry HT Capabilities.
    pub ht: bool,
    /// Its BSS is 40 MHz wide with the secondary channel above: its HT
    /// Capabilities admit 40 MHz and an HT Operation names the offset.
    pub ht40: bool,
    /// Its BSS is HE with this BSS color: its beacons and Probe Responses
    /// carry HT Capabilities, HE Capabilities admitting MCS 0-9 and an HE
    /// Operation.
    pub he_bss_color: Option<u8>,
    /// The WMM Parameter Element of its Association Response, in place of
    /// the WMM Information element.
    pub wmm_association: Option<Vec<u8>>,
    replay_counter: u64,
}

impl ScriptedAp {
    pub fn new(security: ApSecurity) -> Self {
        Self {
            security,
            seen: 0,
            outbox: VecDeque::new(),
            sequence: 0,
            probe_requests: Vec::new(),
            probe_destinations: Vec::new(),
            answers_probes: true,
            authentications: 0,
            associations: 0,
            eapol_from_station: Vec::new(),
            handshake_complete: false,
            uplink: Vec::new(),
            nulls: Vec::new(),
            actions: Vec::new(),
            deauthenticated: false,
            next_beacon_micros: None,
            tim_unicast: false,
            beacons_sent: 0,
            tsf_offset: 1_000_000,
            sae_commit: None,
            sae_keys: None,
            pmk: match security {
                ApSecurity::Wpa2Psk => Some(Pmk::derive(PASSPHRASE, SSID).unwrap()),
                ApSecurity::Open | ApSecurity::Sae => None,
            },
            ptk: None,
            wmm_beacon: None,
            ht: false,
            ht40: false,
            he_bss_color: None,
            wmm_association: None,
            replay_counter: 0,
        }
    }

    /// A Group Message 1 that replaces the group key with `key_id`, every
    /// octet `key`, whose receive sequence counter is `rsc`.
    pub fn group_message1(&mut self, key_id: u8, key: u8, rsc: [u8; 8]) -> Vec<u8> {
        let ptk = self.ptk.as_ref().expect("a completed handshake");
        let mut kde = [key; 24];
        kde[..8].copy_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, key_id, 0]);
        let wrapped = software_aes128_key_wrap(ptk.kek(), &kde).unwrap();
        self.replay_counter += 1;
        RsnTxFrame::<512>::group_message1(
            self.akm(),
            STA,
            self.replay_counter,
            rsc,
            wrapped.as_bytes(),
        )
        .unwrap()
        .authenticate(ptk)
        .as_bytes()
        .to_vec()
    }

    /// `eapol` to the station in a data MPDU: under the pairwise key with
    /// `packet_number`, or in the clear without one.
    pub fn eapol_to_station(&mut self, eapol: &[u8], packet_number: Option<u64>) -> Vec<u8> {
        self.data(None, packet_number, false, EAPOL, eapol, AP)
    }

    /// The access point's SAE commit, drawn from `seed`.
    pub fn with_sae_commit(mut self, commit: SaeCommit) -> Self {
        self.sae_commit = Some(commit);
        self
    }

    fn take_sequence(&mut self) -> u16 {
        let sequence = self.sequence;
        self.sequence = (self.sequence + 1) & 0x0fff;
        sequence
    }

    fn rsn(&self) -> Option<&'static [u8]> {
        match self.security {
            ApSecurity::Open => None,
            ApSecurity::Wpa2Psk => Some(&RSN_PSK),
            ApSecurity::Sae => Some(&RSN_SAE),
        }
    }

    fn akm(&self) -> Akm {
        match self.security {
            ApSecurity::Sae => Akm::Sae,
            ApSecurity::Open | ApSecurity::Wpa2Psk => Akm::Psk,
        }
    }

    /// Queue one frame for the station.
    pub fn queue(&mut self, frame: Vec<u8>) {
        let protected = frame[1] & 0x40 != 0;
        self.outbox.push_back((frame, meta(protected)));
    }

    /// Read every attempt the station submitted since the last read.
    pub fn absorb(&mut self, model: &Model) {
        let submitted = model.submitted();
        while self.seen < submitted.len() {
            let attempt = &submitted[self.seen];
            self.seen += 1;
            for frame in &attempt.frames {
                self.observe(model, frame, attempt.key);
            }
        }
    }

    /// One step: read what the station submitted, send the TBTT and beacon
    /// that are due, and deliver one queued frame. Whether anything
    /// happened.
    pub fn step(&mut self, model: &Model, now_micros: u64) -> bool {
        let mut progress = false;
        let submitted = model.submitted();
        while self.seen < submitted.len() {
            let attempt = &submitted[self.seen];
            self.seen += 1;
            progress = true;
            for frame in &attempt.frames {
                self.observe(model, frame, attempt.key);
            }
        }
        if let Some(due) = self.next_beacon_micros
            && now_micros >= due
            && model.queued_events() == 0
        {
            self.next_beacon_micros = Some(due + u64::from(BEACON_INTERVAL_TU) * 1_024);
            model.fire_tbtt(VifId(0));
            let beacon = self.beacon(0x80, [0xff; 6], due);
            self.outbox.push_front((beacon, meta(false)));
            self.beacons_sent += 1;
            return true;
        }
        if model.queued_events() == 0
            && let Some((frame, meta)) = self.outbox.pop_front()
        {
            if on_channel(model) {
                model.receive(&frame, meta);
            }
            progress = true;
        }
        progress
    }

    fn observe(&mut self, model: &Model, frame: &[u8], key: KeySelector) {
        let frame_control = frame[0];
        match frame_control {
            // Probe Request.
            0x40 => {
                self.probe_requests
                    .push(model.channel().map_or(0, |channel| channel.number()));
                self.probe_destinations.push(
                    frame[4..10]
                        .try_into()
                        .expect("a Probe Request's address 1"),
                );
                if on_channel(model) && self.answers_probes {
                    let response = self.beacon(0x50, STA, 0);
                    self.outbox.push_back((response, meta(false)));
                }
            }
            // Authentication.
            0xb0 => {
                self.authentications += 1;
                let algorithm = u16::from_le_bytes([frame[24], frame[25]]);
                if algorithm == 0 {
                    let mut response = self.management(0xb0, STA);
                    response.extend_from_slice(&[0, 0, 2, 0, 0, 0]);
                    self.outbox.push_back((response, meta(false)));
                } else {
                    self.sae(frame);
                }
            }
            // Association Request.
            0x00 => {
                self.associations += 1;
                let mut response = self.management(0x10, STA);
                let capability: u16 = 0x0001 | if self.rsn().is_some() { 0x0010 } else { 0 };
                response.extend_from_slice(&capability.to_le_bytes());
                response.extend_from_slice(&0_u16.to_le_bytes());
                response.extend_from_slice(&(ASSOCIATION_ID | 0xc000).to_le_bytes());
                response.extend_from_slice(&[1, RATES.len() as u8]);
                response.extend_from_slice(&RATES);
                // A WMM element: the access point takes QoS data.
                match &self.wmm_association {
                    Some(element) => response.extend_from_slice(element),
                    None => response
                        .extend_from_slice(&[221, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00]),
                }
                self.outbox.push_back((response, meta(false)));
                if self.rsn().is_some() {
                    self.replay_counter += 1;
                    let message1 =
                        RsnTxFrame::<512>::message1(self.akm(), STA, self.replay_counter, ANONCE)
                            .unwrap();
                    let data = self.data(None, None, false, EAPOL, message1.as_bytes(), AP);
                    self.outbox.push_back((data, meta(false)));
                }
            }
            // Deauthentication.
            0xc0 => self.deauthenticated = true,
            // Action, with or without protection.
            0xd0 => {
                let start = if frame[1] & 0x40 != 0 { 32 } else { 24 };
                self.actions.push(frame[start..].to_vec());
            }
            // Null Data: the Power Management bit.
            0x48 => self.nulls.push(frame[1] & 0x10 != 0),
            // Data and QoS Data.
            0x08 | 0x88 => self.uplink(frame, key),
            _ => {}
        }
    }

    fn uplink(&mut self, frame: &[u8], key: KeySelector) {
        let qos = frame[0] == 0x88;
        let protected = frame[1] & 0x40 != 0;
        let mut offset = if qos { 26 } else { 24 };
        if protected {
            offset += 8;
        }
        let ether_type = u16::from_be_bytes([frame[offset + 6], frame[offset + 7]]);
        let payload = frame[offset + 8..].to_vec();
        if ether_type == EAPOL {
            self.eapol(&payload);
            return;
        }
        self.uplink.push(Uplink {
            destination: frame[16..22].try_into().unwrap(),
            ether_type,
            payload,
            qos_tid: qos.then(|| frame[24] & 0x0f),
            sequence: u16::from_le_bytes([frame[22], frame[23]]) >> 4,
            protected,
            key,
        });
    }

    fn eapol(&mut self, eapol: &[u8]) {
        let key_frame = EapolKeyFrame::parse(eapol).unwrap();
        let message = key_frame.message();
        self.eapol_from_station.push(message);
        match message {
            EapolKeyMessage::PairwiseMessage2 => {
                let supplicant_nonce = *key_frame.nonce();
                let pmk = match self.security {
                    ApSecurity::Sae => {
                        Pmk::from_bytes(self.sae_keys.as_ref().expect("an SAE exchange").pmk)
                    }
                    _ => self.pmk.as_ref().expect("a PSK").duplicate(),
                };
                let ptk = pmk.derive_ptk(
                    self.akm(),
                    PtkContext {
                        authenticator_address: AP,
                        supplicant_address: STA,
                        authenticator_nonce: ANONCE,
                        supplicant_nonce,
                    },
                );
                let rsn = OwnedRsnIe::<64>::try_copy(self.rsn().unwrap()).unwrap();
                let gtk = RsnGtk::new(1, false, GTK).unwrap();
                let igtk = (self.security == ApSecurity::Sae)
                    .then(|| RsnIgtk::new(4, [0; 6], IGTK).unwrap());
                let plain =
                    RsnPlainKeyData::<128>::build(rsn.as_bytes(), &gtk, igtk.as_ref()).unwrap();
                let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes()).unwrap();
                self.replay_counter += 1;
                let message3 = RsnTxFrame::<512>::message3(
                    self.akm(),
                    STA,
                    self.replay_counter,
                    ANONCE,
                    [0; 8],
                    wrapped.as_bytes(),
                )
                .unwrap()
                .authenticate(&ptk);
                let data = self.data(None, None, false, EAPOL, message3.as_bytes(), AP);
                self.outbox.push_back((data, meta(false)));
                self.ptk = Some(ptk);
            }
            EapolKeyMessage::PairwiseMessage4 => self.handshake_complete = true,
            _ => {}
        }
    }

    fn sae(&mut self, frame: &[u8]) {
        let transaction = u16::from_le_bytes([frame[26], frame[27]]);
        let body = &frame[30..];
        let commit = self.sae_commit.as_ref().expect("an SAE commit");
        if transaction == 1 {
            let station = SaeCommitValues::parse(body, false).unwrap();
            self.sae_keys = Some(commit.process(station).unwrap());
            let mut reply = [0; SAE_COMMIT_LEN];
            commit.values().encode(None, false, &mut reply).unwrap();
            let frame = self.sae_frame(1, &reply);
            self.outbox.push_back((frame, meta(false)));
        } else {
            let keys = self.sae_keys.as_ref().unwrap();
            assert_eq!(keys.verify_peer_confirm(body), Ok(1));
            let confirm = keys.own_confirm(1);
            let frame = self.sae_frame(2, &confirm);
            self.outbox.push_back((frame, meta(false)));
        }
    }

    fn sae_frame(&mut self, transaction: u16, body: &[u8]) -> Vec<u8> {
        let mut frame = self.management(0xb0, STA);
        frame.extend_from_slice(&3_u16.to_le_bytes());
        frame.extend_from_slice(&transaction.to_le_bytes());
        frame.extend_from_slice(&0_u16.to_le_bytes());
        frame.extend_from_slice(body);
        frame
    }

    /// A management header from the access point.
    pub fn management(&mut self, frame_control: u8, destination: [u8; 6]) -> Vec<u8> {
        let mut frame = vec![0_u8; 24];
        frame[0] = frame_control;
        frame[4..10].copy_from_slice(&destination);
        frame[10..16].copy_from_slice(&AP);
        frame[16..22].copy_from_slice(&AP);
        let sequence = self.take_sequence();
        frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
        frame
    }

    /// A beacon (`0x80`) or Probe Response (`0x50`).
    pub fn beacon(&mut self, frame_control: u8, destination: [u8; 6], tsf: u64) -> Vec<u8> {
        let mut frame = self.management(frame_control, destination);
        frame.extend_from_slice(&(tsf + self.tsf_offset).to_le_bytes());
        frame.extend_from_slice(&BEACON_INTERVAL_TU.to_le_bytes());
        let capability: u16 = 0x0001 | if self.rsn().is_some() { 0x0010 } else { 0 };
        frame.extend_from_slice(&capability.to_le_bytes());
        frame.extend_from_slice(&[0, SSID.len() as u8]);
        frame.extend_from_slice(SSID);
        frame.extend_from_slice(&[1, RATES.len() as u8]);
        frame.extend_from_slice(&RATES);
        frame.extend_from_slice(&[3, 1, AP_CHANNEL]);
        if frame_control == 0x80 {
            // TIM: DTIM count 0 of period 1; the station's AID bit when
            // traffic waits for it.
            let bitmap = if self.tim_unicast {
                1 << ASSOCIATION_ID
            } else {
                0
            };
            frame.extend_from_slice(&[5, 4, 0, 1, 0, bitmap]);
        }
        if let Some(rsn) = self.rsn() {
            frame.extend_from_slice(rsn);
        }
        if self.ht40 {
            let mut capabilities = HT_CAPABILITIES;
            // Supported Channel Width Set: 20 and 40 MHz.
            capabilities[2] |= 1 << 1;
            frame.extend_from_slice(&capabilities);
            // HT Operation: the primary channel, the secondary above and
            // any channel width allowed.
            let mut operation = [0_u8; 24];
            operation[..4].copy_from_slice(&[61, 22, AP_CHANNEL, 0x05]);
            frame.extend_from_slice(&operation);
        } else if self.ht || self.he_bss_color.is_some() {
            frame.extend_from_slice(&HT_CAPABILITIES);
        }
        if let Some(color) = self.he_bss_color {
            frame.extend_from_slice(&HE_CAPABILITIES);
            // HE Operation: no parameters, the BSS color, HE-MCS 0-9 on one
            // stream.
            frame.extend_from_slice(&[255, 7, 36, 0, 0, 0, color, 0xfd, 0xff]);
        }
        if let Some(wmm) = &self.wmm_beacon {
            frame.extend_from_slice(wmm);
        }
        frame
    }

    /// A data MPDU to the station: QoS when `tid` is given, protected with
    /// packet number `packet_number` when given.
    pub fn data(
        &mut self,
        tid: Option<u8>,
        packet_number: Option<u64>,
        retry: bool,
        ether_type: u16,
        payload: &[u8],
        source: [u8; 6],
    ) -> Vec<u8> {
        let sequence = self.take_sequence();
        self.data_with_sequence(
            sequence,
            tid,
            packet_number,
            retry,
            ether_type,
            payload,
            source,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn data_with_sequence(
        &mut self,
        sequence: u16,
        tid: Option<u8>,
        packet_number: Option<u64>,
        retry: bool,
        ether_type: u16,
        payload: &[u8],
        source: [u8; 6],
    ) -> Vec<u8> {
        let mut frame = vec![0_u8; 24];
        frame[0] = if tid.is_some() { 0x88 } else { 0x08 };
        frame[1] =
            0x02 | if packet_number.is_some() { 0x40 } else { 0 } | if retry { 0x08 } else { 0 };
        frame[4..10].copy_from_slice(&STA);
        frame[10..16].copy_from_slice(&AP);
        frame[16..22].copy_from_slice(&source);
        frame[22..24].copy_from_slice(&(sequence << 4).to_le_bytes());
        if let Some(tid) = tid {
            frame.extend_from_slice(&[tid, 0]);
        }
        if let Some(packet_number) = packet_number {
            let header = CcmpHeader::new(
                CcmpPacketNumber::new(packet_number).unwrap(),
                CcmpKeyId::new(0).unwrap(),
            );
            frame.extend_from_slice(&header.encode());
        }
        frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
        frame.extend_from_slice(&ether_type.to_be_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    /// An ADDBA Request for `tid` with `window` starting at `start`.
    pub fn addba_request(&mut self, tid: u8, window: u16, start: u16) -> Vec<u8> {
        let mut frame = self.management(0xd0, STA);
        let parameters: u16 = 0x0002 | (u16::from(tid) << 2) | (window << 6);
        frame.extend_from_slice(&[3, 0, 7]);
        frame.extend_from_slice(&parameters.to_le_bytes());
        frame.extend_from_slice(&0_u16.to_le_bytes());
        frame.extend_from_slice(&(start << 4).to_le_bytes());
        frame
    }

    /// A BlockAckReq for `tid` starting at `start`.
    pub fn block_ack_request(&self, tid: u8, start: u16) -> Vec<u8> {
        let mut frame = vec![0x84, 0, 0, 0];
        frame.extend_from_slice(&STA);
        frame.extend_from_slice(&AP);
        frame.extend_from_slice(&((u16::from(tid) << 12) | 0x0004).to_le_bytes());
        frame.extend_from_slice(&(start << 4).to_le_bytes());
        frame
    }

    /// A Deauthentication with `reason`.
    pub fn deauthentication(&mut self, reason: u16) -> Vec<u8> {
        let mut frame = self.management(0xc0, STA);
        frame.extend_from_slice(&reason.to_le_bytes());
        frame
    }

    /// A broadcast Deauthentication protected with BIP under the IGTK of
    /// Message 3, its IPN the transmitter's next.
    pub fn bip_deauthentication(
        &mut self,
        bip: &mut oer_ieee80211_rsn::bip::BipTransmitter,
        reason: u16,
    ) -> Vec<u8> {
        let mut frame = self.management(0xc0, [0xff; 6]);
        frame.extend_from_slice(&reason.to_le_bytes());
        let length = frame.len();
        frame.resize(
            length + oer_ieee80211_rsn::bip::MANAGEMENT_MIC_ELEMENT_LEN,
            0,
        );
        let protected = bip.protect(&mut frame, length).unwrap();
        frame.truncate(protected);
        frame
    }
}

/// The BIP transmitter of the access point's IGTK, the one an SAE
/// association's Message 3 delivers.
pub fn bip_transmitter() -> oer_ieee80211_rsn::bip::BipTransmitter {
    oer_ieee80211_rsn::bip::BipTransmitter::new(&RsnIgtk::new(4, [0; 6], IGTK).unwrap())
}

/// The access point hears the station and is heard only on its primary
/// channel, at 20 or 40 MHz.
fn on_channel(model: &Model) -> bool {
    model
        .channel()
        .is_some_and(|channel| channel.band() == Band::Ghz2_4 && channel.number() == AP_CHANNEL)
}

/// Metadata of a frame the backend received; a protected one was
/// decrypted and verified.
/// A WMM Parameter Element of `count` whose records, by ACI (best effort,
/// background, video, voice), are `(AIFSN, ECWmin, ECWmax, TXOP)`.
pub fn wmm_parameter_element(count: u8, records: [(u8, u8, u8, u16); 4]) -> Vec<u8> {
    let mut element = vec![221, 24, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, count & 0x0f, 0];
    for (aci, (aifsn, ecw_min, ecw_max, txop)) in records.into_iter().enumerate() {
        element.push(((aci as u8) << 5) | aifsn);
        element.push((ecw_max << 4) | ecw_min);
        element.extend_from_slice(&txop.to_le_bytes());
    }
    element
}

/// An HE Capabilities element admitting HE-MCS 0-9 both ways on one
/// stream at 20 MHz.
pub const HE_CAPABILITIES: [u8; 24] = [
    255, 22, 35, 0x03, 0x18, 0x9c, 0xca, 0x10, 0x80, 0x00, 0x10, 0x8a, 0x1b, 0x0d, 0xc0, 0x1f,
    0x00, 0x02, 0x82, 0x01, 0xfd, 0xff, 0xfd, 0xff,
];

/// An HT Capabilities element: MCS 0-7, A-MPDU up to 64 KiB.
pub const HT_CAPABILITIES: [u8; 28] = [
    45, 26, 0x00, 0x00, 0x17, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0,
];

/// An ADDBA Response body to `dialog_token` for `tid` with `status`.
pub fn addba_response(dialog_token: u8, tid: u8, status: u16, window: u16) -> Vec<u8> {
    let parameters: u16 = (1 << 1) | (u16::from(tid) << 2) | (window << 6);
    let mut body = vec![3, 1, dialog_token];
    body.extend_from_slice(&status.to_le_bytes());
    body.extend_from_slice(&parameters.to_le_bytes());
    body.extend_from_slice(&0_u16.to_le_bytes());
    body
}

pub fn meta(protected: bool) -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(AP_CHANNEL, ChannelWidth::Mhz20).unwrap());
    meta.rssi_dbm = RxEvidence::HardwareObserved(-40);
    meta.noise_floor_dbm = RxEvidence::HardwareObserved(-96);
    meta.crypto = RxEvidence::HardwareObserved(if protected {
        RxCryptoStatus::DecryptedAndIntegrityVerified
    } else {
        RxCryptoStatus::Unprotected
    });
    meta
}
