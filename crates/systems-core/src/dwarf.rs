use gimli::{ColumnType, Dwarf, EndianSlice, Reader, RunTimeEndian};
use object::{Object, ObjectSection};

use crate::SourceLocation;

const MAX_DEBUG_SECTION: usize = 16 * 1024 * 1024;
const MAX_DEBUG_TOTAL: usize = 64 * 1024 * 1024;
const MAX_SOURCE_ROWS: usize = 200_000;
const MAX_UNITS: usize = 16_384;
const MAX_PATH: usize = 4096;
const MAX_PATH_TEXT: usize = 16 * 1024 * 1024;

pub(super) fn read_locations(
    object: &object::File<'_>,
    warnings: &mut Vec<String>,
) -> Vec<SourceLocation> {
    match read(object) {
        Ok(locations) => locations,
        Err(error) => {
            warnings.push(format!("Source mapping unavailable: {error}"));
            Vec::new()
        }
    }
}

fn read(object: &object::File<'_>) -> Result<Vec<SourceLocation>, String> {
    for section in object.sections() {
        let name = section.name().unwrap_or_default();
        if name.starts_with(".zdebug_") {
            return Err("compressed DWARF is not supported".into());
        }
        if name.ends_with(".dwo") {
            return Err("split DWARF is not supported".into());
        }
    }
    let endian = if object.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };
    let mut total = 0usize;
    let dwarf = Dwarf::load(|id| -> Result<_, String> {
        let Some(section) = object.section_by_name(id.name()) else {
            return Ok(EndianSlice::new(&[], endian));
        };
        if let object::SectionFlags::Elf { sh_flags } = section.flags()
            && sh_flags & u64::from(object::elf::SHF_COMPRESSED) != 0
        {
            return Err("compressed DWARF is not supported".into());
        }
        if section.size() > MAX_DEBUG_SECTION as u64 {
            return Err("debug section exceeds the 16 MiB analysis limit".into());
        }
        let data = section.data().map_err(|e| e.to_string())?;
        total = total
            .checked_add(data.len())
            .ok_or("Debug data size overflows")?;
        if total > MAX_DEBUG_TOTAL {
            return Err("debug data exceeds the 64 MiB analysis limit".into());
        }
        Ok(EndianSlice::new(data, endian))
    })?;
    let mut result = Vec::new();
    let mut units = dwarf.units();
    let mut unit_count = 0;
    let mut row_count = 0;
    let mut path_bytes = 0;
    while let Some(header) = units.next().map_err(|e| e.to_string())? {
        unit_count += 1;
        if unit_count > MAX_UNITS {
            return Err("DWARF exceeds the 16,384 compilation-unit limit".into());
        }
        let unit = dwarf.unit(header).map_err(|e| e.to_string())?;
        let Some(program) = unit.line_program.clone() else {
            continue;
        };
        let mut rows = program.rows();
        let mut previous: Option<SourceLocation> = None;
        while let Some((header, row)) = rows.next_row().map_err(|e| e.to_string())? {
            row_count += 1;
            if row_count > MAX_SOURCE_ROWS {
                return Err("DWARF exceeds the 200,000 line-row analysis limit".into());
            }
            if let Some(mut previous) = previous.take() {
                if row.address() < previous.address {
                    return Err("DWARF line sequence moves backwards".into());
                }
                if row.address() > previous.address {
                    previous.end_address = row.address();
                    path_bytes += previous.path.len();
                    if path_bytes > MAX_PATH_TEXT {
                        return Err("source paths exceed the 16 MiB metadata text limit".into());
                    }
                    result.push(previous);
                }
            }
            if row.end_sequence() {
                continue;
            }
            let Some(line) = row.line().and_then(|line| u32::try_from(line.get()).ok()) else {
                continue;
            };
            let Some(file) = row.file(header) else {
                continue;
            };
            let filename = dwarf
                .attr_string(&unit, file.path_name())
                .map_err(|e| e.to_string())?;
            let filename = reader_string(filename)?;
            if filename.is_empty() {
                continue;
            }
            let directory = file
                .directory(header)
                .map(|value| dwarf.attr_string(&unit, value).map_err(|e| e.to_string()))
                .transpose()?
                .map(reader_string)
                .transpose()?
                .unwrap_or_default();
            let compilation_directory = unit
                .comp_dir
                .map(reader_string)
                .transpose()?
                .unwrap_or_default();
            let path = source_path(&compilation_directory, &directory, &filename)?;
            let column = match row.column() {
                ColumnType::LeftEdge => 0,
                ColumnType::Column(column) => u32::try_from(column.get()).unwrap_or(u32::MAX),
            };
            previous = Some(SourceLocation {
                path,
                line,
                column,
                address: row.address(),
                file_offset: None,
                end_address: row.address(),
            });
        }
        if previous.is_some() {
            return Err("DWARF line sequence has no end marker".into());
        }
    }
    Ok(result)
}

fn reader_string<R: Reader>(reader: R) -> Result<String, String> {
    use gimli::ReaderOffset;
    if reader.len().into_u64() > MAX_PATH as u64 {
        return Err("DWARF path exceeds 4,096 bytes".into());
    }
    reader
        .to_string()
        .map(|text| text.into_owned())
        .map_err(|e| e.to_string())
}

// DWARF can describe a different operating system than this host. Preserve
// its spelling and recognize both Unix roots and Windows drive/UNC paths.
fn source_path(compilation_directory: &str, directory: &str, file: &str) -> Result<String, String> {
    fn absolute(path: &str) -> bool {
        path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':')
    }
    let path = if absolute(file) {
        file.to_string()
    } else if absolute(directory) || compilation_directory.is_empty() {
        join(directory, file)
    } else {
        join(&join(compilation_directory, directory), file)
    };
    if path.len() > MAX_PATH {
        return Err("Combined DWARF source path exceeds 4,096 bytes".into());
    }
    Ok(path)
}

fn join(directory: &str, file: &str) -> String {
    if directory.is_empty() {
        file.into()
    } else if file.is_empty() {
        directory.into()
    } else {
        format!("{}/{file}", directory.trim_end_matches(['/', '\\']))
    }
}

#[cfg(test)]
mod tests {
    use super::source_path;

    #[test]
    fn paths_use_compile_directory_without_host_filesystem_access() {
        assert_eq!(
            source_path("/project", "src", "main.c").unwrap(),
            "/project/src/main.c"
        );
        assert_eq!(
            source_path("/project", "/other", "main.c").unwrap(),
            "/other/main.c"
        );
        assert_eq!(
            source_path("/project", "src", "C:\\build\\main.c").unwrap(),
            "C:\\build\\main.c"
        );
    }
}
