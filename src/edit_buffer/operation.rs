use crate::command::Address;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Op {
    Inverse(Box<Op>),
    Append(Insertion),
    Delete(DeleteData),
    Edit(EditData),
    Insert(Insertion),
}

#[derive(Debug, Clone, Default)]
pub struct DeleteData {
    pub address: Option<Address>,
    pub lines_removed: Vec<String>,
    pub current_line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct EditData {
    pub filename: PathBuf,
    pub current_line: usize,
    pub lines_removed: Vec<String>,
    pub clean_fingerprint: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct Insertion {
    pub address: Option<Address>,
    pub current_line: usize,
    pub lines: Vec<String>,
}

impl Op {
    pub fn inverse(&self) -> Op {
        match self {
            Op::Inverse(op) => *op.clone(),
            _ => Op::Inverse(Box::new(self.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_op() {
        let op = Op::Append(Insertion {
            address: None,
            current_line: 1,
            lines: Vec::new(),
        });
        let inv = op.inverse();
        assert!(match inv {
            Op::Inverse(bo) => matches!(*bo, Op::Append(_)),
            _ => false,
        });
    }

    #[test]
    fn inverse_inverse_op() {
        let op = Op::Append(Insertion {
            address: None,
            current_line: 1,
            lines: Vec::new(),
        });
        let inv = op.inverse();
        let inv_inv = inv.inverse();
        assert!(matches!(inv_inv, Op::Append(_)));
    }
}
