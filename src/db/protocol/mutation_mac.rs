use toasty::Model;

#[derive(Debug, Model)]
#[unique(name, index_mac)]
pub struct MutationMac {
    #[key]
    #[auto]
    pub id: i64,
    pub name: String,
    pub index_mac: String, // Stored as hex; the store layer encodes/decodes the raw index bytes.
    pub value_mac: Vec<u8>,
}
