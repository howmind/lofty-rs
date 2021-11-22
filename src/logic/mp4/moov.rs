use super::atom_info::AtomInfo;
#[cfg(feature = "mp4_atoms")]
use super::ilst::read::parse_ilst;
use super::ilst::{AtomIdent, Ilst};
use super::read::skip_unneeded;
use super::trak::Trak;
use crate::error::{LoftyError, Result};

use std::io::{Read, Seek};

use byteorder::{BigEndian, ReadBytesExt};

pub(crate) struct Moov {
	pub(crate) traks: Vec<Trak>,
	// Represents a parsed moov.udta.meta.ilst since we don't need anything else
	pub(crate) meta: Option<Ilst>,
}

impl Moov {
	pub(crate) fn find<R>(data: &mut R) -> Result<AtomInfo>
	where
		R: Read + Seek,
	{
		let mut moov = (false, None);

		while let Ok(atom) = AtomInfo::read(data) {
			if atom.ident == AtomIdent::Fourcc(*b"moov") {
				moov = (true, Some(atom));
				break;
			}

			skip_unneeded(data, atom.extended, atom.len)?;
		}

		if !moov.0 {
			return Err(LoftyError::Mp4("No \"moov\" atom found"));
		}

		Ok(moov.1.unwrap())
	}

	pub(crate) fn parse<R>(data: &mut R) -> Result<Self>
	where
		R: Read + Seek,
	{
		let mut traks = Vec::new();
		let mut meta = None;

		while let Ok(atom) = AtomInfo::read(data) {
			if let AtomIdent::Fourcc(fourcc) = atom.ident {
				match &fourcc {
					b"trak" => traks.push(Trak::parse(data, &atom)?),
					b"udta" => {
						meta = meta_from_udta(data, atom.len - 8)?;
					},
					_ => skip_unneeded(data, atom.extended, atom.len)?,
				}

				continue;
			}

			skip_unneeded(data, atom.extended, atom.len)?
		}

		Ok(Self { traks, meta })
	}
}

fn meta_from_udta<R>(data: &mut R, len: u64) -> Result<Option<Ilst>>
where
	R: Read + Seek,
{
	let mut read = 8;
	let mut meta = (false, 0_u64);

	while read < len {
		let atom = AtomInfo::read(data)?;

		if atom.ident == AtomIdent::Fourcc(*b"meta") {
			meta = (true, atom.len);
			break;
		}

		read += atom.len;
		skip_unneeded(data, atom.extended, atom.len)?;
	}

	if !meta.0 {
		return Ok(None);
	}

	// The meta atom has 4 bytes we don't care about
	// Version (1)
	// Flags (3)
	let _version_flags = data.read_u32::<BigEndian>()?;

	read = 12;
	let mut islt = (false, 0_u64);

	while read < meta.1 {
		let atom = AtomInfo::read(data)?;

		if atom.ident == AtomIdent::Fourcc(*b"ilst") {
			islt = (true, atom.len);
			break;
		}

		read += atom.len;
		skip_unneeded(data, atom.extended, atom.len)?;
	}

	#[cfg(feature = "mp4_atoms")]
	if islt.0 {
		return parse_ilst(data, islt.1 - 8);
	}

	Ok(None)
}
