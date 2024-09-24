
use error_stack::Report;
use error_stack::Result;
use error_stack::ResultExt;

use super::database::Database;
use super::database::DatabaseReader;

use super::database::DatabaseWriter;
use super::error::Error;

use super::model::lowlevel;
use super::model;
use super::model::metadata; 

impl<'a> Database<'a> {
    pub(super) fn init_meta_unsafe() -> Result<(model::Metadata, model::Metadata), Error> {
        let meta = model::Metadata {
            magic: lowlevel::MAGIC,
            version: lowlevel::VERSION,
            address: 0,
            mapsize: 1048576, // Do know what this is
            main: model::Database {
                pad: 4096,
                flags: model::metadata::Flags::empty(),
                depth: 0,
                branch_pages: 0,
                leaf_pages: 0,
                overflow_pages: 0,
                entries: 0,
                root: 0,
            },
            free: model::Database {
                pad: 4096,
                flags: model::metadata::Flags::INTEGERKEY,
                depth: 0,
                branch_pages: 0,
                leaf_pages: 0,
                overflow_pages: 0,
                entries: 0,
                root: 0,
            },
            last_pgno: 0,
            txnid: 0,
        };
        Ok((meta.clone(), meta.clone()))
    }

    pub(super) fn write_page_header_unsafe<'b>(writer: &'b mut (dyn DatabaseWriter + 'a), header: model::Header) -> Result<(), Error> {
        writer.write_word(header.pageno as u64)?;
        writer.write_u16(header.pad)?;
        writer.write_u16(header.flags.bits())?;
        writer.write_u16(header.free_lower)?;
        writer.write_u16(header.free_upper)?;
        Ok(())
    }

    pub(super) fn write_db_unsafe<'b>(writer: &'b mut (dyn DatabaseWriter + 'a), db: metadata::Database) -> Result<(), Error> {
        writer.write_u32(db.pad)?;
        writer.write_u16(db.flags.bits())?;
        writer.write_u16(db.depth)?;
        writer.write_word(db.branch_pages)?;
        writer.write_word(db.leaf_pages)?;
        writer.write_word(db.overflow_pages)?;
        writer.write_word(db.entries)?;
        writer.write_word(db.root)?;
        Ok(())
    }

    pub(super) fn write_leaf_unsafe<'b>(writer: &'b mut (dyn DatabaseWriter + 'a), leaf: model::Leaf) -> Result<(), Error> {
        writer.seek(std::io::SeekFrom::Start((leaf.pageno as u64) * 4096))?;

        let head = writer.pos()?;
        tracing::debug!("leaf pos: {}", head);

        let nkeys = leaf.nodes.len();
        
        let mut ptrs = Vec::<usize>::new();
        let mut offset = 4096;
        for i in 0..nkeys {
            let node = &leaf.nodes[i];
            offset = offset - (4 + 2 + 2 +node.data.len() + node.key.len());
            ptrs.push(offset);
        }

        writer.write_word(leaf.pageno as u64)?;
        writer.write_u16(0)?;
        writer.write_u16(leaf.flags.bits())?;
        let pos = writer.pos()?;
        writer.write_u16(((nkeys<<1) + (pos-head + 4)) as u16)?;
        writer.write_u16(offset as u16)?;
        for ptr in ptrs {
            writer.write_u16(ptr as u16)?;
        }

        let tail = writer.pos()?;
        let fill = offset - (tail-head);
        writer.write_fill(fill)?;

        for node in leaf.nodes {
            writer.write_u32(node.data.len() as u32)?;
            writer.write_u16(node.flags)?;
            writer.write_u16(node.key.len() as u16)?;
            writer.write_exact(&node.key)?;
            writer.write_exact(&node.data)?;
        }
        
        Ok(())
    }

    pub(super) fn write_meta_unsafe<'b>(writer: &'b mut (dyn DatabaseWriter + 'a), meta: model::Metadata, pageno: usize) -> Result<(), Error> {
        let head = pageno * 4096;
        writer.seek(std::io::SeekFrom::Start(head as u64))?;
        Self::write_page_header_unsafe(writer, model::Header {
            pageno: 0,
            pad: 0,
            flags: model::header::Flags::META,
            free_lower: 0,
            free_upper: 0,
        })?;

        writer.write_u32(meta.magic)?;
        writer.write_u32(meta.version)?;
        writer.write_word(meta.address)?;
        writer.write_word(meta.mapsize)?;

        Self::write_db_unsafe(writer, meta.free)?;
        Self::write_db_unsafe(writer, meta.main)?;

        writer.write_word(meta.last_pgno)?;
        writer.write_word(meta.txnid)?;

        let tail = writer.pos()?;
        let fill = 4096 - (tail-head);
        writer.write_fill(fill)?;

        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use std::sync::Once;
    use tempfile;

    use crate::lmdb::factory::Factory;
    use crate::lmdb::writer::Writer32;
    use crate::lmdb::writer::Writer64;

    use crate::lmdb::reader::Reader32;
    use crate::lmdb::reader::Reader64;

    use super::*;
    use super::super::model;

    macro_rules! test_case {
        ($fname:expr) => {
            std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/",
                $fname
            ))
        };
    }

    static INIT: Once = Once::new();

    pub fn setup() -> () { 
        INIT.call_once(|| {
            tracing_subscriber::fmt::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .init();
        });
    }

    #[test]
    fn test_write_meta_64() {
        setup();
        let file = tempfile::NamedTempFile::new().unwrap();
        let writer = std::io::BufWriter::new(file.reopen().unwrap());
        let mut writer = Writer64::from(writer);
        let dw = &mut writer;

        let (meta1, meta2) = Database::init_meta_unsafe().unwrap();
        Database::write_meta_unsafe(dw, meta1, 0).unwrap();
        Database::write_meta_unsafe(dw, meta2, 1).unwrap();
        writer.flush().unwrap();

        // Try to read back
        let file = file.reopen().unwrap();
        let reader = std::io::BufReader::new(file);
        let mut reader = Reader64::from(reader);
        let dr = &mut reader;

        let meta = Database::pick_meta_unsafe(dr).unwrap();
        tracing::debug!("Metadata: {:?}", meta);
    }

    #[test]
    fn test_write_leaf_64() {
        setup();
        let file = tempfile::NamedTempFile::new().unwrap();
        let writer = std::io::BufWriter::new(file.reopen().unwrap());
        let mut writer = Writer64::from(writer);
        let dw = &mut writer;

        let (meta1, meta2) = Database::init_meta_unsafe().unwrap();
        Database::write_meta_unsafe(dw, meta1, 0).unwrap();
        Database::write_meta_unsafe(dw, meta2, 1).unwrap();

        let mut nodes = Vec::<model::Node>::new();
        for i in 1..3 {
            nodes.push(model::Node {
                flags: 0,
                key: vec![i; 1],
                data: vec![2*i;1],
            });
        }
        Database::write_leaf_unsafe(dw, model::Leaf {
            pageno: 2,
            flags: model::header::Flags::LEAF,
            nodes,
        }).unwrap();
        writer.flush().unwrap();

        // Try to read back
        let file = file.reopen().unwrap();
        let reader = std::io::BufReader::new(file);
        let mut reader = Reader64::from(reader);
        let dr = &mut reader;

        Database::seek_page_unsafe(dr, 2).unwrap();
        let leaf = Database::read_leaf_unsafe(dr).unwrap();
        tracing::debug!("{:#?}", leaf);
    }

    #[test]
    fn test_write_meta_32() {
        setup();
        let file = tempfile::NamedTempFile::new().unwrap();
        let writer = std::io::BufWriter::new(file.reopen().unwrap());
        let mut writer = Writer32::from(writer);
        let dw = &mut writer;

        let (meta1, meta2) = Database::init_meta_unsafe().unwrap();
        Database::write_meta_unsafe(dw, meta1, 0).unwrap();
        Database::write_meta_unsafe(dw, meta2, 1).unwrap();

        writer.flush().unwrap();

        // Try to read back
        let file = file.reopen().unwrap();
        let reader = std::io::BufReader::new(file);
        let mut reader = Reader32::from(reader);
        let dr = &mut reader;

        let meta = Database::pick_meta_unsafe(dr).unwrap();
        tracing::debug!("Metadata: {:?}", meta);  
    }
    
}
            