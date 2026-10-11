//! Flattens the ESP32-C3 SVD into an address-sorted register list.
//!
//! Deliberately strict: any SVD construct this file does not use today (clusters,
//! register-level `derivedFrom`, comma-list `dimIndex`, a `size` other than 32 bits, a
//! `resetValue` or `dim` on a peripheral, a derived peripheral with registers of its own,
//! `dim` = 0) is an error, never a silent skip. Mechanics and occurrence counts for the pinned
//! file: `docs/research/esp32c3-svd.md`, sections 2.2 and 2.3.
use roxmltree::{Document, Node};

/// Every register of the pinned SVD is 32 bits wide (research report, section 2.2).
const REGISTER_SIZE_BITS: u32 = 0x20;

/// One 32-bit register at its absolute address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub addr: u32,
    pub peripheral: String,
    pub name: String,
    pub reset_value: u32,
    /// `false` when the SVD gives no register-level `resetValue`: the value is then the
    /// device default (0), which means "unknown" rather than "known zero".
    pub reset_explicit: bool,
}

/// Two SVD registers that are the same physical register under two names: the one
/// named here is kept, the other dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alias {
    pub addr: u32,
    pub peripheral: &'static str,
    pub register: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error(
        "the SVD is not well-formed XML ({detail}); re-download it from the URL in PROVENANCE.md"
    )]
    Xml { detail: String },
    #[error(
        "<{element}> in {context} has no <{child}>; the SVD layout changed, review the generator before bumping the file"
    )]
    Missing {
        element: &'static str,
        context: String,
        child: &'static str,
    },
    #[error(
        "cannot read the number `{text}`; the generator accepts decimal and 0x-prefixed hexadecimal only"
    )]
    Number { text: String },
    #[error(
        "unsupported SVD construct in {context}: {what}; extend the generator instead of skipping it"
    )]
    Unsupported { context: String, what: String },
    #[error(
        "peripheral {child} derives from {parent}, which is not defined before it; check the derivedFrom attribute"
    )]
    UnknownParent { child: String, parent: String },
    #[error(
        "{first} and {second} are both at {addr:#010x}; add an alias rule naming the register to keep"
    )]
    Collision {
        first: String,
        second: String,
        addr: u32,
    },
    #[error(
        "{registers} are all at {addr:#010x}; an alias rule can only choose between two registers, so review the generator before bumping the SVD"
    )]
    CrowdedAddress { registers: String, addr: u32 },
    #[error(
        "the alias rule for {peripheral}.{register} at {addr:#010x} matches no address collision; remove it"
    )]
    UnusedAlias {
        peripheral: &'static str,
        register: &'static str,
        addr: u32,
    },
}

/// Expands `derivedFrom` peripherals and `dim` register arrays of `svd_xml`.
///
/// The result is sorted by address and every address appears once: `aliases` decides
/// which register survives where two share an address, any other collision is an error.
///
/// # Errors
///
/// Returns a [`GenError`] when the XML is malformed, a required element is missing, or
/// the SVD uses a construct the generator does not support.
pub fn expand(svd_xml: &str, aliases: &[Alias]) -> Result<Vec<Register>, GenError> {
    let document = Document::parse(svd_xml).map_err(|e| GenError::Xml {
        detail: e.to_string(),
    })?;
    let device = document.root_element();
    let default_reset = child_text(device, "resetValue")
        .map(parse_number)
        .transpose()?
        .unwrap_or(0);
    let peripherals = child(device, "peripherals").ok_or_else(|| GenError::Missing {
        element: "device",
        context: "the SVD".to_owned(),
        child: "peripherals",
    })?;

    let mut registers = Vec::new();
    let mut seen: Vec<Node> = Vec::new();
    for peripheral in peripherals
        .children()
        .filter(|n| n.has_tag_name("peripheral"))
    {
        expand_peripheral(peripheral, &seen, default_reset, &mut registers)?;
        seen.push(peripheral);
    }
    resolve_collisions(registers, aliases)
}

fn expand_peripheral<'a, 'input>(
    peripheral: Node<'a, 'input>,
    seen: &[Node<'a, 'input>],
    default_reset: u32,
    out: &mut Vec<Register>,
) -> Result<(), GenError> {
    let name = required_text(peripheral, "peripheral", "the peripheral list", "name")?;
    let base = parse_number(required_text(
        peripheral,
        "peripheral",
        name,
        "baseAddress",
    )?)?;
    reject_peripheral_level_settings(peripheral, name)?;
    let source = register_source(peripheral, name, seen)?;
    let Some(registers) = child(source, "registers") else {
        return Ok(());
    };
    for element in registers.children().filter(Node::is_element) {
        if !element.has_tag_name("register") {
            return Err(GenError::Unsupported {
                context: name.to_owned(),
                what: format!("<{}> inside <registers>", element.tag_name().name()),
            });
        }
        expand_register(element, name, base, default_reset, out)?;
    }
    Ok(())
}

/// Settings the expansion would otherwise ignore: SVD lets a peripheral carry its own `resetValue`
/// (overriding the device default for its registers) or a `dim` (an array of peripherals), and a
/// derived peripheral may add registers to its parent's. None of this occurs in the pinned file.
fn reject_peripheral_level_settings(peripheral: Node, name: &str) -> Result<(), GenError> {
    let unsupported = |what: &str| GenError::Unsupported {
        context: name.to_owned(),
        what: what.to_owned(),
    };
    if child(peripheral, "resetValue").is_some() {
        return Err(unsupported("a peripheral-level <resetValue>"));
    }
    if child(peripheral, "dim").is_some() {
        return Err(unsupported("a peripheral-level <dim>"));
    }
    if peripheral.attribute("derivedFrom").is_some() && child(peripheral, "registers").is_some() {
        return Err(unsupported(
            "a derived peripheral with <registers> of its own",
        ));
    }
    Ok(())
}

/// The element whose `<registers>` a peripheral uses: itself, or its `derivedFrom`
/// parent (one hop; the pinned SVD has no chained derivation).
fn register_source<'a, 'input>(
    peripheral: Node<'a, 'input>,
    name: &str,
    seen: &[Node<'a, 'input>],
) -> Result<Node<'a, 'input>, GenError> {
    let Some(parent_name) = peripheral.attribute("derivedFrom") else {
        return Ok(peripheral);
    };
    let parent = seen
        .iter()
        .copied()
        .find(|p| child_text(*p, "name") == Some(parent_name))
        .ok_or_else(|| GenError::UnknownParent {
            child: name.to_owned(),
            parent: parent_name.to_owned(),
        })?;
    if parent.attribute("derivedFrom").is_some() {
        return Err(GenError::Unsupported {
            context: name.to_owned(),
            what: format!("chained derivation through {parent_name}"),
        });
    }
    Ok(parent)
}

fn expand_register(
    register: Node,
    peripheral: &str,
    base: u32,
    default_reset: u32,
    out: &mut Vec<Register>,
) -> Result<(), GenError> {
    let name = required_text(register, "register", peripheral, "name")?;
    let context = format!("{peripheral}.{name}");
    if register.attribute("derivedFrom").is_some() {
        return Err(GenError::Unsupported {
            context,
            what: "register-level derivedFrom".to_owned(),
        });
    }
    let offset = parse_number(required_text(
        register,
        "register",
        &context,
        "addressOffset",
    )?)?;
    let size = parse_number(required_text(register, "register", &context, "size")?)?;
    if size != REGISTER_SIZE_BITS {
        return Err(GenError::Unsupported {
            context,
            what: format!(
                "register size {size:#x} bits, only {REGISTER_SIZE_BITS:#x} is supported"
            ),
        });
    }
    let explicit_reset = child_text(register, "resetValue")
        .map(parse_number)
        .transpose()?;

    for (instance_name, delta) in instances(register, name, &context)? {
        let addr = base
            .checked_add(offset)
            .and_then(|a| a.checked_add(delta))
            .ok_or_else(|| GenError::Unsupported {
                context: context.clone(),
                what: "address overflows 32 bits".to_owned(),
            })?;
        out.push(Register {
            addr,
            peripheral: peripheral.to_owned(),
            name: instance_name,
            reset_value: explicit_reset.unwrap_or(default_reset),
            reset_explicit: explicit_reset.is_some(),
        });
    }
    Ok(())
}

/// The `(name, byte offset from the first element)` of each element a register stands
/// for: one for a plain register, `dim` for an array.
fn instances(register: Node, name: &str, context: &str) -> Result<Vec<(String, u32)>, GenError> {
    let Some(dim_text) = child_text(register, "dim") else {
        return Ok(vec![(name.to_owned(), 0)]);
    };
    let dim = parse_number(dim_text)?;
    if dim == 0 {
        return Err(GenError::Unsupported {
            context: context.to_owned(),
            what: "dim = 0, an array of no register".to_owned(),
        });
    }
    let increment = parse_number(required_text(
        register,
        "register",
        context,
        "dimIncrement",
    )?)?;
    let indices = dim_indices(register, context, dim)?;
    let mut elements = Vec::with_capacity(indices.len());
    for (position, index) in (0..dim).zip(indices) {
        let delta = increment
            .checked_mul(position)
            .ok_or_else(|| GenError::Unsupported {
                context: context.to_owned(),
                what: "array offset overflows 32 bits".to_owned(),
            })?;
        elements.push((name.replace("[%s]", &index).replace("%s", &index), delta));
    }
    Ok(elements)
}

/// Array indices: `0..dim` when `<dimIndex>` is absent, else the `a-b` range it names.
fn dim_indices(register: Node, context: &str, dim: u32) -> Result<Vec<String>, GenError> {
    let Some(text) = child_text(register, "dimIndex") else {
        return Ok((0..dim).map(|i| i.to_string()).collect());
    };
    let unsupported = |what: String| GenError::Unsupported {
        context: context.to_owned(),
        what,
    };
    let (first, last) = text
        .split_once('-')
        .filter(|_| !text.contains(','))
        .ok_or_else(|| unsupported(format!("dimIndex `{text}` is not an `a-b` range")))?;
    let (first, last) = (parse_decimal(first)?, parse_decimal(last)?);
    let count = last.checked_sub(first).and_then(|span| span.checked_add(1));
    if count != Some(dim) {
        return Err(unsupported(format!(
            "dimIndex `{text}` does not hold dim={dim} indices"
        )));
    }
    Ok((first..=last).map(|i| i.to_string()).collect())
}

/// Sorts by address, then keeps one register per address: the one an alias rule names.
fn resolve_collisions(
    mut registers: Vec<Register>,
    aliases: &[Alias],
) -> Result<Vec<Register>, GenError> {
    registers
        .sort_by(|a, b| (a.addr, &a.peripheral, &a.name).cmp(&(b.addr, &b.peripheral, &b.name)));
    let mut resolved: Vec<Register> = Vec::with_capacity(registers.len());
    let mut used = vec![false; aliases.len()];
    let mut pending = registers.into_iter().peekable();
    while let Some(first) = pending.next() {
        let addr = first.addr;
        let mut group = vec![first];
        while let Some(next) = pending.next_if(|r| r.addr == addr) {
            group.push(next);
        }
        resolved.push(pick_survivor(group, aliases, &mut used)?);
    }
    // A rule that resolved nothing is stale (the SVD changed, or the rule was mistyped).
    if let Some(index) = used.iter().position(|&was_used| !was_used) {
        let alias = &aliases[index];
        return Err(GenError::UnusedAlias {
            peripheral: alias.peripheral,
            register: alias.register,
            addr: alias.addr,
        });
    }
    Ok(resolved)
}

/// The register that keeps an address shared by `group` (never empty). A group of one
/// needs no rule; a group of two needs an alias naming one of its members (marked in `used`);
/// a larger group is an error whatever the rules say.
fn pick_survivor(
    mut group: Vec<Register>,
    aliases: &[Alias],
    used: &mut [bool],
) -> Result<Register, GenError> {
    let label = |r: &Register| format!("{}.{}", r.peripheral, r.name);
    if group.len() == 1 {
        return Ok(group.swap_remove(0));
    }
    if group.len() > 2 {
        return Err(GenError::CrowdedAddress {
            registers: group.iter().map(label).collect::<Vec<_>>().join(", "),
            addr: group[0].addr,
        });
    }
    let named_by_alias = group.iter().enumerate().find_map(|(member, r)| {
        let rule = aliases.iter().position(|a| {
            a.addr == r.addr && a.peripheral == r.peripheral && a.register == r.name
        })?;
        Some((member, rule))
    });
    if let Some((member, rule)) = named_by_alias {
        used[rule] = true;
        return Ok(group.swap_remove(member));
    }
    // INVARIANT: this point is reached only with `group.len() >= 2`, the single-element
    // case having returned above, so indices 0 and 1 exist.
    Err(GenError::Collision {
        first: label(&group[0]),
        second: label(&group[1]),
        addr: group[0].addr,
    })
}

fn child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|c| c.has_tag_name(tag))
}

fn child_text<'a>(node: Node<'a, '_>, tag: &str) -> Option<&'a str> {
    child(node, tag).and_then(|c| c.text()).map(str::trim)
}

fn required_text<'a>(
    node: Node<'a, '_>,
    element: &'static str,
    context: &str,
    tag: &'static str,
) -> Result<&'a str, GenError> {
    child_text(node, tag).ok_or_else(|| GenError::Missing {
        element,
        context: context.to_owned(),
        child: tag,
    })
}

/// Decimal or `0x`-prefixed hexadecimal, as SVD writes them.
fn parse_number(text: &str) -> Result<u32, GenError> {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| GenError::Number {
        text: text.to_owned(),
    })
}

fn parse_decimal(text: &str) -> Result<u32, GenError> {
    text.trim().parse().map_err(|_| GenError::Number {
        text: text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(peripherals: &str) -> String {
        format!(
            "<device><name>T</name><resetValue>0x00000000</resetValue><peripherals>{peripherals}</peripherals></device>"
        )
    }

    fn reg(name: &str, offset: &str, extra: &str) -> String {
        format!(
            "<register><name>{name}</name><addressOffset>{offset}</addressOffset><size>0x20</size>{extra}</register>"
        )
    }

    fn periph(name: &str, base: &str, registers: &str) -> String {
        format!(
            "<peripheral><name>{name}</name><baseAddress>{base}</baseAddress><registers>{registers}</registers></peripheral>"
        )
    }

    fn names(regs: &[Register]) -> Vec<(u32, String)> {
        regs.iter()
            .map(|r| (r.addr, format!("{}.{}", r.peripheral, r.name)))
            .collect()
    }

    #[test]
    fn plain_register_gets_absolute_address_and_explicit_reset() {
        let xml = device(&periph(
            "UART0",
            "0x60000000",
            &reg("STATUS", "0x1C", "<resetValue>0xE000C000</resetValue>"),
        ));
        let regs = expand(&xml, &[]).unwrap();
        assert_eq!(regs.len(), 1);
        assert_eq!(
            (regs[0].addr, regs[0].reset_value, regs[0].reset_explicit),
            (0x6000_001C, 0xE000_C000, true)
        );
    }

    #[test]
    fn missing_reset_value_inherits_the_device_default_and_is_flagged() {
        let xml = device(&periph("GPIO", "0x60004000", &reg("STRAP", "0x38", "")));
        let regs = expand(&xml, &[]).unwrap();
        assert_eq!((regs[0].reset_value, regs[0].reset_explicit), (0, false));
    }

    #[test]
    fn dim_array_expands_percent_s_with_a_range_index() {
        let extra = "<dim>3</dim><dimIncrement>0x10</dimIncrement><dimIndex>0-2</dimIndex>";
        let xml = device(&periph(
            "DMA",
            "0x6003F000",
            &reg("INT_RAW_CH%s", "0x0", extra),
        ));
        assert_eq!(
            names(&expand(&xml, &[]).unwrap()),
            [
                (0x6003_F000, "DMA.INT_RAW_CH0".into()),
                (0x6003_F010, "DMA.INT_RAW_CH1".into()),
                (0x6003_F020, "DMA.INT_RAW_CH2".into())
            ]
        );
    }

    #[test]
    fn dim_array_without_index_drops_the_brackets() {
        let extra = "<dim>2</dim><dimIncrement>0x4</dimIncrement>";
        let xml = device(&periph(
            "AES",
            "0x6003A000",
            &reg("IV_MEM[%s]", "0x50", extra),
        ));
        assert_eq!(
            names(&expand(&xml, &[]).unwrap()),
            [
                (0x6003_A050, "AES.IV_MEM0".into()),
                (0x6003_A054, "AES.IV_MEM1".into())
            ]
        );
    }

    #[test]
    fn derived_peripheral_copies_the_parent_registers_at_its_own_base() {
        let parent = periph("UART0", "0x60000000", &reg("FIFO", "0x0", ""));
        let child = "<peripheral derivedFrom=\"UART0\"><name>UART1</name><baseAddress>0x60010000</baseAddress></peripheral>";
        let regs = expand(&device(&format!("{parent}{child}")), &[]).unwrap();
        assert_eq!(
            names(&regs),
            [
                (0x6000_0000, "UART0.FIFO".into()),
                (0x6001_0000, "UART1.FIFO".into())
            ]
        );
    }

    #[test]
    fn output_is_sorted_by_address_whatever_the_document_order() {
        let regs = format!("{}{}", reg("B", "0x8", ""), reg("A", "0x4", ""));
        let out = expand(&device(&periph("P", "0x60000000", &regs)), &[]).unwrap();
        assert_eq!(
            out.iter().map(|r| r.addr).collect::<Vec<_>>(),
            [0x6000_0004, 0x6000_0008]
        );
    }

    #[test]
    fn listed_alias_keeps_the_winner_and_drops_the_other_register() {
        let a = periph("APB_CTRL", "0x60026000", &reg("RND_DATA", "0xB0", ""));
        let b = periph("RNG", "0x60026000", &reg("DATA", "0xB0", ""));
        let alias = [Alias {
            addr: 0x6002_60B0,
            peripheral: "RNG",
            register: "DATA",
        }];
        let regs = expand(&device(&format!("{a}{b}")), &alias).unwrap();
        assert_eq!(names(&regs), [(0x6002_60B0, "RNG.DATA".into())]);
    }

    #[test]
    fn unlisted_address_collision_is_an_error_naming_both_registers() {
        let a = periph("A", "0x60000000", &reg("X", "0x0", ""));
        let b = periph("B", "0x60000000", &reg("Y", "0x0", ""));
        let err = expand(&device(&format!("{a}{b}")), &[])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("A.X") && err.contains("B.Y") && err.contains("0x60000000"),
            "{err}"
        );
    }

    #[test]
    fn unsupported_constructs_are_rejected_instead_of_being_skipped() {
        let cluster = "<peripheral><name>P</name><baseAddress>0x0</baseAddress><registers><cluster><name>C</name></cluster></registers></peripheral>";
        assert!(matches!(
            expand(&device(cluster), &[]),
            Err(GenError::Unsupported { .. })
        ));
        let wide = periph(
            "P",
            "0x0",
            "<register><name>R</name><addressOffset>0x0</addressOffset><size>0x10</size></register>",
        );
        assert!(matches!(
            expand(&device(&wide), &[]),
            Err(GenError::Unsupported { .. })
        ));
        let orphan = "<peripheral derivedFrom=\"NOPE\"><name>Q</name><baseAddress>0x0</baseAddress></peripheral>";
        assert!(matches!(
            expand(&device(orphan), &[]),
            Err(GenError::UnknownParent { .. })
        ));
    }

    #[test]
    fn numbers_accept_hex_and_decimal_and_reject_garbage() {
        assert_eq!(parse_number("0x1C").unwrap(), 0x1C);
        assert_eq!(parse_number("28").unwrap(), 28);
        assert!(parse_number("twelve").is_err());
    }
    #[test]
    fn malformed_xml_and_a_missing_peripheral_list_are_reported() {
        assert!(matches!(expand("<device>", &[]), Err(GenError::Xml { .. })));
        assert!(matches!(
            expand("<device><name>T</name></device>", &[]),
            Err(GenError::Missing {
                child: "peripherals",
                ..
            })
        ));
    }

    #[test]
    fn a_register_without_size_or_name_is_reported_with_its_context() {
        let no_size = "<register><name>R</name><addressOffset>0x0</addressOffset></register>";
        let err = expand(&device(&periph("P", "0x0", no_size)), &[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("P.R") && err.contains("<size>"), "{err}");
        let no_name = "<register><addressOffset>0x0</addressOffset></register>";
        assert!(matches!(
            expand(&device(&periph("P", "0x0", no_name)), &[]),
            Err(GenError::Missing { child: "name", .. })
        ));
    }

    #[test]
    fn derivation_chains_and_register_level_derivation_are_rejected() {
        let parent = periph("A", "0x0", &reg("R", "0x0", ""));
        let middle = "<peripheral derivedFrom=\"A\"><name>B</name><baseAddress>0x100</baseAddress></peripheral>";
        let leaf = "<peripheral derivedFrom=\"B\"><name>C</name><baseAddress>0x200</baseAddress></peripheral>";
        assert!(matches!(
            expand(&device(&format!("{parent}{middle}{leaf}")), &[]),
            Err(GenError::Unsupported { .. })
        ));
        let derived_register = "<register derivedFrom=\"X\"><name>R</name></register>";
        assert!(matches!(
            expand(&device(&periph("P", "0x0", derived_register)), &[]),
            Err(GenError::Unsupported { .. })
        ));
    }

    #[test]
    fn dim_index_forms_other_than_a_matching_range_are_rejected() {
        let dim = |index: &str| {
            let extra =
                format!("<dim>3</dim><dimIncrement>0x4</dimIncrement><dimIndex>{index}</dimIndex>");
            device(&periph("P", "0x0", &reg("R%s", "0x0", &extra)))
        };
        assert!(matches!(
            expand(&dim("A,B,C"), &[]),
            Err(GenError::Unsupported { .. })
        ));
        assert!(matches!(
            expand(&dim("0-5"), &[]),
            Err(GenError::Unsupported { .. })
        ));
        assert!(matches!(
            expand(&dim("5-0"), &[]),
            Err(GenError::Unsupported { .. })
        ));
        assert!(matches!(
            expand(&dim("x-y"), &[]),
            Err(GenError::Number { .. })
        ));
        assert!(matches!(
            expand(&dim("7"), &[]),
            Err(GenError::Unsupported { .. })
        ));
    }

    #[test]
    fn a_dim_array_needs_its_increment_and_must_fit_in_32_bits() {
        let no_increment = device(&periph("P", "0x0", &reg("R%s", "0x0", "<dim>2</dim>")));
        assert!(matches!(
            expand(&no_increment, &[]),
            Err(GenError::Missing {
                child: "dimIncrement",
                ..
            })
        ));
        let overflow = device(&periph("P", "0xFFFFFFF0", &reg("R", "0x20", "")));
        assert!(matches!(
            expand(&overflow, &[]),
            Err(GenError::Unsupported { .. })
        ));
        let big_step = "<dim>3</dim><dimIncrement>0xFFFFFFFF</dimIncrement>";
        let stride = device(&periph("P", "0x0", &reg("R%s", "0x0", big_step)));
        assert!(matches!(
            expand(&stride, &[]),
            Err(GenError::Unsupported { .. })
        ));
    }

    #[test]
    fn an_unreadable_reset_value_names_the_offending_text() {
        let xml = device(&periph(
            "P",
            "0x0",
            &reg("R", "0x0", "<resetValue>0xZZ</resetValue>"),
        ));
        let err = expand(&xml, &[]).unwrap_err().to_string();
        assert!(err.contains("0xZZ"), "{err}");
    }

    fn assert_unsupported(xml: &str, expected: &str) {
        match expand(xml, &[]) {
            Err(GenError::Unsupported { what, .. }) => {
                assert!(what.contains(expected), "{what}");
            }
            other => panic!("expected an unsupported construct, got {other:?}"),
        }
    }

    #[test]
    fn a_peripheral_level_reset_value_is_rejected_not_ignored() {
        let xml = format!(
            "<peripheral><name>P</name><baseAddress>0x0</baseAddress><resetValue>0x1</resetValue><registers>{}</registers></peripheral>",
            reg("R", "0x0", "")
        );
        assert_unsupported(&device(&xml), "<resetValue>");
    }

    #[test]
    fn a_peripheral_level_dim_is_rejected_not_ignored() {
        let xml = format!(
            "<peripheral><name>P%s</name><baseAddress>0x0</baseAddress><dim>2</dim><dimIncrement>0x100</dimIncrement><registers>{}</registers></peripheral>",
            reg("R", "0x0", "")
        );
        assert_unsupported(&device(&xml), "<dim>");
    }

    #[test]
    fn a_derived_peripheral_with_registers_of_its_own_is_rejected() {
        let parent = periph("A", "0x0", &reg("R", "0x0", ""));
        let child = format!(
            "<peripheral derivedFrom=\"A\"><name>B</name><baseAddress>0x100</baseAddress><registers>{}</registers></peripheral>",
            reg("EXTRA", "0x4", "")
        );
        assert_unsupported(&device(&format!("{parent}{child}")), "derived peripheral");
    }

    #[test]
    fn a_zero_sized_array_is_rejected() {
        let extra = "<dim>0</dim><dimIncrement>0x4</dimIncrement>";
        assert_unsupported(
            &device(&periph("P", "0x0", &reg("R%s", "0x0", extra))),
            "dim = 0",
        );
    }

    #[test]
    fn three_registers_at_one_address_are_an_error_even_when_an_alias_names_one() {
        let at = |name: &str, register: &str| periph(name, "0x60000000", &reg(register, "0x0", ""));
        let xml = device(&format!("{}{}{}", at("A", "X"), at("B", "Y"), at("C", "Z")));
        let alias = [Alias {
            addr: 0x6000_0000,
            peripheral: "B",
            register: "Y",
        }];
        let err = expand(&xml, &alias).unwrap_err();
        assert!(matches!(err, GenError::CrowdedAddress { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("A.X") && text.contains("B.Y") && text.contains("C.Z"),
            "{text}"
        );
    }

    #[test]
    fn an_alias_rule_that_resolves_no_collision_is_an_error_telling_to_remove_it() {
        let xml = device(&periph("A", "0x60000000", &reg("X", "0x0", "")));
        let alias = [Alias {
            addr: 0x6000_0000,
            peripheral: "A",
            register: "X",
        }];
        let err = expand(&xml, &alias).unwrap_err();
        assert!(matches!(err, GenError::UnusedAlias { .. }), "{err}");
        assert!(err.to_string().contains("remove it"), "{err}");
    }
}
