use crate::{LoftyError, Picture, Result};

use crate::components::logic::constants::{VORBIS_COMMENT_HEAD, VORBIS_SETUP_HEAD};

use byteorder::{LittleEndian, ReadBytesExt};
use std::borrow::Cow;
use std::collections::HashMap;

use crate::components::logic::ogg::{is_metadata, page_from_packet};
use ogg_pager::Page;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

pub(crate) fn create_pages(
	file: &mut File,
	sig: &[u8],
	vendor: &str,
	comments: &HashMap<String, String>,
	pictures: &Option<Cow<'static, [Picture]>>,
) -> Result<()> {
	let mut packet = Vec::new();

	packet.extend(sig.iter());
	packet.extend((vendor.len() as u32).to_le_bytes().iter());
	packet.extend(vendor.as_bytes().iter());

	let comments_len = pictures.as_ref().map_or_else(
		|| comments.len() as u32,
		|pictures| (comments.len() + pictures.len()) as u32,
	);

	packet.extend(comments_len.to_le_bytes().iter());

	let mut comment_str = Vec::new();

	for (a, b) in comments {
		comment_str.push(format!("{}={}", a, b));
		let last = comment_str.last().unwrap();
		let len = last.as_bytes().len() as u32;
		packet.extend(len.to_le_bytes().iter());
		packet.extend(last.as_bytes().iter());
	}

	if let Some(pics) = pictures {
		for pic in pics.iter() {
			comment_str.push(format!(
				"METADATA_BLOCK_PICTURE={}",
				base64::encode(pic.as_apic_bytes())
			));
			let last = comment_str.last().unwrap();
			let len = last.as_bytes().len() as u32;
			packet.extend(len.to_le_bytes().iter());
			packet.extend(last.as_bytes().iter());
		}
	}

	let mut pages = page_from_packet(&mut *packet)?;
	write_to(file, &mut pages, sig)?;

	Ok(())
}

fn vorbis_write(
	mut data: &mut File,
	writer: &mut Vec<u8>,
	first_md_content: Vec<u8>,
	ser: u32,
	pages: &mut [Page],
) -> Result<()> {
	let mut remaining = Vec::new();

	let reached_md_end: bool;
	let mut c = Cursor::new(first_md_content);

	c.seek(SeekFrom::Start(7))?;

	let vendor_len = c.read_u32::<LittleEndian>()?;
	c.seek(SeekFrom::Current(i64::from(vendor_len)))?;

	let total_comments = c.read_u32::<LittleEndian>()?;
	let comments_pos = c.seek(SeekFrom::Current(0))?;

	c.seek(SeekFrom::End(0))?;

	loop {
		let p = Page::read(&mut data)?;

		if p.header_type != 1 {
			data.seek(SeekFrom::Start(p.start as u64))?;
			data.read_to_end(&mut remaining)?;

			reached_md_end = true;
			break;
		}

		c.write_all(&p.content)?;
	}

	if !reached_md_end {
		return Err(LoftyError::InvalidData("OGG file ends with comment header"));
	}

	c.seek(SeekFrom::Start(comments_pos))?;

	for _ in 0..total_comments {
		let len = c.read_u32::<LittleEndian>()?;
		c.seek(SeekFrom::Current(i64::from(len)))?;
	}

	if c.read_u8()? != 1 {
		return Err(LoftyError::InvalidData(
			"OGG Vorbis file is missing a framing bit",
		));
	}

	let mut header_ident = [0; 7];
	c.read_exact(&mut header_ident)?;

	if header_ident != VORBIS_SETUP_HEAD {
		return Err(LoftyError::InvalidData(
			"OGG Vorbis file is missing setup header",
		));
	}

	c.seek(SeekFrom::Current(-7))?;

	let mut setup = Vec::new();
	c.read_to_end(&mut setup)?;

	let pages_len = pages.len() - 1;

	for (i, mut p) in pages.iter_mut().enumerate() {
		p.serial = ser;

		if i == pages_len {
			p.content.push(1);
			let mut seg_table = Vec::new();
			seg_table.extend(p.segments().iter());
			seg_table.extend(ogg_pager::segments(&*setup));

			let mut seg_table_len = seg_table.len();

			if seg_table_len > 255 {
				seg_table = seg_table.split_at(255).0.to_vec();
				seg_table_len = 255;
			}

			seg_table.insert(0, seg_table_len as u8);

			let page = p.extend(&*setup);

			let mut p_bytes = p.as_bytes();
			let seg_count = p_bytes[26] as usize;

			p_bytes.splice(26..27 + seg_count, seg_table);
			p_bytes.splice(22..26, ogg_pager::crc32(&*p_bytes).to_le_bytes().to_vec());

			writer.write_all(&*p_bytes)?;

			if let Some(mut page) = page {
				page.serial = ser;
				page.gen_crc();

				writer.write_all(&*page.as_bytes())?;
			}

			break;
		}

		p.gen_crc();
		writer.write_all(&*p.as_bytes())?;
	}

	writer.write_all(&*remaining)?;

	Ok(())
}

fn write_to(mut data: &mut File, pages: &mut [Page], sig: &[u8]) -> Result<()> {
	let vorbis = sig == VORBIS_COMMENT_HEAD;
	let first_page = Page::read(&mut data)?;

	let ser = first_page.serial;

	let mut writer = Vec::new();
	writer.write_all(&*first_page.as_bytes())?;

	let first_md_page = Page::read(&mut data)?;
	is_metadata(&first_md_page, sig)?;

	if vorbis {
		vorbis_write(data, &mut writer, first_md_page.content, ser, pages)?;
	} else {
		let reached_md_end: bool;
		let mut remaining = Vec::new();

		loop {
			let p = Page::read(&mut data)?;

			if p.header_type != 1 {
				data.seek(SeekFrom::Start(p.start as u64))?;
				reached_md_end = true;
				break;
			}
		}

		if !reached_md_end {
			return Err(LoftyError::InvalidData("OGG file ends with comment header"));
		}

		data.read_to_end(&mut remaining)?;

		for mut p in pages.iter_mut() {
			p.serial = ser;
			p.gen_crc();

			writer.write_all(&*p.as_bytes())?;
		}

		writer.write_all(&*remaining)?;
	};

	data.seek(SeekFrom::Start(0))?;
	data.set_len(0)?;
	data.write_all(&*writer)?;

	Ok(())
}