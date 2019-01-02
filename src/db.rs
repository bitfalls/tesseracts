use web3::types::{Address,Block,H256,U256,U128,BlockId,Transaction,TransactionId,BlockNumber,Bytes};
use rocksdb::{DB,Direction,DBIterator,IteratorMode};
use model::*;
use std::iter;
use rand::{Rng, thread_rng};
use rand::distributions::Alphanumeric;
use serde_cbor::{from_slice,to_vec};

#[derive(Copy,Clone,PartialEq)]
#[repr(u8)]
enum RecordType {
    TxLink = 1,
    NextBlock = 2,
    Tx = 3,
    Block = 4,
}

#[derive(Copy,Clone,PartialEq)]
#[repr(u8)]
enum TxLinkType {
    In = 1,
    Out = 2,
    InOut = 3
}


pub struct AppDB {
    db  : DB,
}

pub struct AddrTxs {
    iter : DBIterator,
    key  : Vec<u8>,
}

impl AddrTxs {
    fn new(iter : DBIterator, key : Vec<u8> ) -> Self {
        AddrTxs{iter,key}
    }
}

impl<'a> Iterator for AddrTxs {
    type Item = H256;

    fn next(& mut self) -> Option<H256> {
        if let Some(kv) = self.iter.next() {
            let key = &*(kv.0);
            if key.len() > self.key.len() && key[..self.key.len()]==self.key[..] {
                // unserialize blockno, txindex 
                return Some(H256::from_slice(&key[self.key.len()+16..]));
            }
        }
        None
    }
}

#[derive(Debug)]
pub enum Error {
    Rocks(rocksdb::Error),
    SerdeCbor(serde_cbor::error::Error)
}
impl PartialEq for Error {
    fn eq(&self, other: &Error) -> bool {
        format!("{:?}",self) == format!("{:?}",other)
    }
}

impl From<rocksdb::Error> for Error {
    fn from(err: rocksdb::Error) -> Self {
        Error::Rocks(err)
    }
}
impl From<serde_cbor::error::Error> for Error {
    fn from(err: serde_cbor::error::Error) -> Self {
        Error::SerdeCbor(err)
    }
}

impl AppDB {

    pub fn open_default(path : &str) -> Result<AppDB, Error> {
        Ok(DB::open_default(path).map(|x| AppDB { db : x })?)
    }
    
    pub fn push_tx(&self, tx : &Transaction) -> Result<(),rocksdb::Error> {

        // store tx
        let mut tx_k = vec![RecordType::Tx as u8];
        tx_k.extend_from_slice(&tx.hash);
        self.db.put(tx_k.as_slice(),to_vec(tx).unwrap().as_slice())?;

        // store addr->tx
        let blockno = tx.block_number.unwrap().low_u64().to_le_bytes();
        let txindex = tx.block_number.unwrap().low_u64().to_le_bytes();

        if let Some(to) = tx.to {
            let mut to_k : Vec<u8> = vec![RecordType::TxLink as u8];
            to_k.extend_from_slice(&to);
            to_k.extend_from_slice(&blockno);
            to_k.extend_from_slice(&txindex);
            to_k.extend_from_slice(&tx.hash);
            
            let link_type = if tx.from==to { TxLinkType::InOut } else { TxLinkType::Out };
            self.db.put(to_k.as_slice(),&[link_type as u8])?;
            if link_type == TxLinkType::InOut  {
                return Ok(());
            }
        }

        let mut from_k : Vec<u8> = vec![RecordType::TxLink as u8];
        from_k.extend_from_slice(&tx.from);
        from_k.extend_from_slice(&blockno);
        from_k.extend_from_slice(&txindex);
        from_k.extend_from_slice(&tx.hash);
        self.db.put(&from_k.to_owned(),&[TxLinkType::In as u8])
    }

    pub fn get_tx(&self, txhash : &H256) -> Result<Option<Transaction>,rocksdb::Error> {
        let mut tx_k = vec![RecordType::Tx as u8];
        tx_k.extend_from_slice(&txhash);
        self.db.get(&tx_k).map(|bytes| {
            bytes.map(|v| from_slice::<Transaction>(&*v).unwrap())
        })
    }

    pub fn push_block(&self, block: &Block<Transaction>) -> Result<(),Error> {
        let mut b_k = vec![RecordType::Block as u8];
        let block_no = (block.number.unwrap().low_u64()).to_le_bytes();
        b_k.extend_from_slice(&block_no);

        Ok(self.db.put(b_k.as_slice(),to_vec(block)?.as_slice())?)
    }

    pub fn get_block(&self, blockno : u64) -> Result<Option<Block<Transaction>>,rocksdb::Error> {
        let mut b_k = vec![RecordType::Block as u8];
        b_k.extend_from_slice(&blockno.to_le_bytes());
        self.db.get(&b_k).map(|bytes| {
            bytes.map(|v| from_slice::<Block<Transaction>>(&*v).unwrap())
        })
    }

    pub fn iter_addr_txs<'a>(&self, addr: &Address) -> AddrTxs {
        let mut key : Vec<u8> = vec![];
        key.push(RecordType::TxLink as u8);
        key.extend_from_slice(addr);
        
        let iter = self.db.iterator(
            IteratorMode::From(&key,Direction::Forward));

        AddrTxs::new(iter,key)
    }

    pub fn get_last_block(&self) -> Result<Option<u64>,Error> {
        Ok(self.db.get(&[RecordType::NextBlock as u8]).map(|bytes| {
            bytes.map(|v| {
                let mut le = [0;8];
                le[..].copy_from_slice(&*v);
                u64::from_le_bytes(le)
            })
        })?)
    }

    pub fn set_last_block(&self, n : u64) -> Result<(),Error> {
        Ok(self.db.put(
            &[RecordType::NextBlock as u8],
            &n.to_le_bytes()
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() -> AppDB {
        let mut rng = thread_rng();
        let chars: String = iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .take(7)
                .collect();
        
        let mut tmpfile= std::env::temp_dir();
        tmpfile.push(chars);
        AppDB::open_default(
            tmpfile.as_os_str().to_str().expect("bad OS filename")
        ).expect("unable to create db")
    }

    #[test]
    fn test_add_and_iter() {
        let appdb = init();
        let one_u128 = U128::from_dec_str("1").unwrap();
        let one_u256 = U256::from_dec_str("1").unwrap();
        let a1 = hex_to_addr("0x1eb983836ea12dc37cc4da2effae9c9fbd0b395a").unwrap();
        let a2 = hex_to_addr("0x1eb983836ea12dc37cc4da2effae9c9fbd0b395b").unwrap();
        let h1 = hex_to_h256("0xd69fc1890a1b2742b5c2834d031e34ba55ef3820d463a8d0a674bb5dd9a3b74b").unwrap();

        let tx = Transaction{
            hash: h1,
            nonce: one_u256,
            block_hash: None,
            block_number: Some(U256::from_dec_str("10").unwrap()),
            transaction_index: Some(one_u128),
            from: a1,
            to: Some(a2),
            value: one_u256,
            gas_price: one_u256,
            gas: one_u256,
            input: Bytes(Vec::new()),            
        };

        assert_eq!(Ok(()),appdb.push_tx(&tx));

        let mut it_a1 = appdb.iter_addr_txs(&a1);
        assert_eq!(Some(h1), it_a1.next());
        assert_eq!(None, it_a1.next());

        let mut it_a2 = appdb.iter_addr_txs(&a2);
        assert_eq!(Some(h1), it_a2.next());
        assert_eq!(None, it_a2.next());
    }

    #[test]
    fn test_set_get_block() {
        let appdb = init();
        assert_eq!(Ok(None), appdb.get_last_block());
        assert_eq!(Ok(()), appdb.set_last_block(1));
        assert_eq!(Ok(Some(1)), appdb.get_last_block());
        assert_eq!(Ok(()), appdb.set_last_block(2));
        assert_eq!(Ok(Some(2)), appdb.get_last_block());
    }
}