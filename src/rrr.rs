use std::{collections::HashMap, fs, path::Path};

use anyhow::{Result, anyhow};

use pest::{
    Parser,
    iterators::{Pair, Pairs},
};
use pest_derive::Parser;

use crate::{
    rule_set::{ConfigOrigin, Pattern, RuleSet, RuleSetBuilder},
    types::ProfileIdentifier,
    utils,
};

pub struct RrrBuilder {
    profiles: HashMap<ProfileIdentifier, RuleSetBuilder>,
    current_profile: ProfileIdentifier,
    case_insensitive: bool,
}

pub struct Rrr {
    profiles: HashMap<ProfileIdentifier, RuleSet>,
}

#[derive(Parser)]
#[grammar = "config.pest"]
struct ConfigParser;

impl Rrr {
    pub fn profile(&self, profile_identifier: &str) -> Result<&RuleSet> {
        /* todo: we have a mismatch here between ProfileIdentifier, &ProfileIdentifier (=&String)
           and &str -> we should get our story straight
        */
        Ok(self
            .profiles
            .get(profile_identifier)
            .ok_or_else(|| anyhow!("Profile '{}' does not exist", profile_identifier))?)
    }
}

impl RrrBuilder {
    pub fn new(case_insensitive: bool) -> Self {
        /* todo: add a `only_profile: Option<ProfileIdentifier>` to only take into account the
           given profile (useful to only parse part of complex configurations).
        */

        let profiles = HashMap::from([(
            "default".to_string(),
            RuleSetBuilder::new("default".to_string(), case_insensitive),
        )]);
        Self {
            profiles,
            current_profile: "default".to_string(),
            case_insensitive,
        }
    }

    /**
    Parse a config file.
    If there are include or import in the config, they will be loaded automatically.
    */
    pub fn config(mut self, file_path: &Path) -> Result<Self> {
        let input = fs::read_to_string(file_path)?;
        let file = ConfigParser::parse(Rule::file, &input)?.next().unwrap();
        for inner in file.into_inner() {
            if inner.as_rule() == Rule::line {
                self = self.parse_line(file_path, inner)?;
            }
        }

        Ok(self)
    }

    fn parse_line(self, file: &Path, line: Pair<Rule>) -> Result<Self> {
        let inner = line.into_inner().next().unwrap(); // meta, alias, invalid, match
        match inner.as_rule() {
            Rule::meta => {
                let mut inners = inner.into_inner();
                let meta = inners.next().unwrap();
                let target = meta.clone().into_inner().next().unwrap();
                match meta.as_rule() {
                    Rule::include => self.parse_meta_include(file, target),
                    Rule::import => self.parse_meta_import(file, target),
                    Rule::profile => self.parse_meta_profile(file, target),
                    _ => unreachable!(),
                }
            }
            Rule::alias => {
                let mut inners = inner.into_inner();
                let (identifier, target) = (inners.next().unwrap(), inners.next().unwrap());
                self.parse_alias(file, identifier, target)
            }
            Rule::r#match => {
                let mut inners = inner.into_inner();
                let (r#match, target) = (inners.next().unwrap(), inners.next().unwrap());
                if target.as_rule() == Rule::invalid_alias {
                    return Err(anyhow!("Invalid alias in match '{}'", target.as_str()));
                }
                self.parse_match(file, r#match, target)
            }
            Rule::invalid => {
                let inner = inner.into_inner().next().unwrap();
                match inner.as_rule() {
                    Rule::invalid_meta => Err(anyhow!("Invalid meta '{}'", inner.as_str())),
                    Rule::invalid_alias => Err(anyhow!("Invalid alias '{}'", inner.as_str())),
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    }

    fn parse_meta_include(self, file: &Path, target: Pair<Rule>) -> Result<Self> {
        let target = parse_string(target)?;
        self.config(Path::new(&target))
    }

    fn parse_meta_import(self, file: &Path, target: Pair<Rule>) -> Result<Self> {
        // todo: check if it's a directory or a file
        // if file => rrs.rule_with_import()
        // if dir => recursively go over each file and rrs.rule_with_import()
        todo!()
    }

    fn parse_meta_profile(mut self, file: &Path, target: Pair<Rule>) -> Result<Self> {
        let target = parse_string(target)?;
        self.profiles
            .entry(target.clone())
            .or_insert(RuleSetBuilder::new(
                target.to_string(),
                self.case_insensitive,
            ));
        self.current_profile = target;
        Ok(self)
    }

    fn parse_alias(
        mut self,
        file: &Path,
        identifier: Pair<Rule>,
        target: Pair<Rule>,
    ) -> Result<Self> {
        let rule_set_builder = self.current_profile();
        let action = parse_string(target)?;

        rule_set_builder.alias(identifier.as_str().to_string(), action);

        Ok(self)
    }

    fn parse_match(mut self, file: &Path, r#match: Pair<Rule>, target: Pair<Rule>) -> Result<Self> {
        let rule_set_builder = self.current_profile();
        let config_origin = token_to_config_origin(file, &r#match);
        let pattern = match_token_to_pattern(&r#match);

        if target.as_rule() == Rule::alias_identifier {
            let alias_identifier = target.as_str().to_string();
            rule_set_builder.rule_with_alias(config_origin, pattern, alias_identifier)?;
        } else {
            let action = parse_string(target)?;
            rule_set_builder.rule_with_action(config_origin, pattern, action);
        }

        Ok(self)
    }

    fn current_profile(&mut self) -> &mut RuleSetBuilder {
        self.profiles
            .get_mut(&self.current_profile)
            .expect("Current profile should exist in the list of profiles")
    }

    pub fn build(self) -> Result<Rrr> {
        let rule_sets: Result<HashMap<ProfileIdentifier, RuleSet>> = self
            .profiles
            .into_iter()
            .map(|(profile_identifier, rule_set_builder)| {
                // Result<V> -> Result<(K, V)> otherwise we end up with (K, Result<V>)
                rule_set_builder
                    .build()
                    .map(|rule_set| (profile_identifier, rule_set))
            })
            .collect();

        Ok(Rrr {
            profiles: rule_sets?,
        })
    }
}

fn parse_string(target: Pair<Rule>) -> Result<String> {
    match target.as_rule() {
        Rule::space_string | Rule::nospace_string => Ok(target.as_str().to_string()),
        Rule::quoted_string => utils::unquote(target.as_str()),
        _ => unreachable!(),
    }
}

fn match_token_to_pattern(r#match: &Pair<Rule>) -> Pattern {
    // fixme: try to avoid the clone() here, into_inner() forces us to own r#match
    let pattern = r#match.clone().into_inner().next().unwrap();

    match r#match.as_rule() {
        Rule::glob_match => Pattern::Glob(pattern.as_str().to_string()),
        Rule::regex_match => Pattern::Regex(pattern.as_str().to_string()),
        _ => unreachable!(),
    }
}

fn token_to_config_origin(file: &Path, r#match: &Pair<Rule>) -> ConfigOrigin {
    let (line, column) = r#match.as_span().start_pos().line_col();
    ConfigOrigin {
        file: file.display().to_string(),
        line,
        column,
    }
}

// todo: remove this method once the parser is OK
fn debug_pair(pair: &Pair<Rule>) -> () {
    println!("Rule::{:?} | text: '{:?}'", pair.as_rule(), pair.as_str())
}

fn debug_pairs(pairs: &Pairs<Rule>) -> () {
    println!("[");
    for inner in pairs.clone() {
        debug_pair(&inner);
    }
    println!("]");
}

/*
todo: add tests for parsing with static conf file
- full config with 3 different profiles in addition to default, test all individual ConfigParser
- invididual test for each config component (note `:import file` vs `:import desktop`)
- test that syntax error are reported
- test invalid states are reported
*/
