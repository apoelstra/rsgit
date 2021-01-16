// Copyright (c) 2021
//      Andrew Poelstra <rsgit@wpsoftware.net>
//
// This program is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation; either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, write to the Free Software
// Foundation, Inc., 675 Mass Ave, Cambridge, MA 02139, USA.
//

mod rust;

use rayon::ThreadPool;
use serde::{de, Deserialize, Deserializer, Serialize};
use std::fmt;
use std::marker::PhantomData;

use crate::git::TempRepo;

/// serde helper from https://stackoverflow.com/a/43627388/14495533
/// to decode strings as single-element vecs of strings. Modified to
/// be generic
fn single_or_seq<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct StringOrVec<T>(PhantomData<Vec<T>>);

    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for StringOrVec<T> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("string or list of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![Deserialize::deserialize(
                de::IntoDeserializer::into_deserializer(value),
            )?])
        }

        fn visit_seq<S>(self, visitor: S) -> Result<Self::Value, S::Error>
        where
            S: de::SeqAccess<'de>,
        {
            Deserialize::deserialize(de::value::SeqAccessDeserializer::new(visitor))
        }
    }

    deserializer.deserialize_any(StringOrVec(PhantomData))
}

#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum Check {
    Rust(self::rust::RustCheck),
}

impl Check {
    pub fn execute(&self, repo: TempRepo, build_pool: &ThreadPool) -> anyhow::Result<Vec<String>> {
        match *self {
            Check::Rust(ref sub) => sub.execute(repo, build_pool),
        }
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Check::Rust(ref sub) => sub.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn decode_rust() {
        let _ck: Check = serde_json::from_str(
            "
            {
                \"type\": \"rust\",
                \"features\": [\"rand\", \"use-serde\", \"base64\"],
                \"version\": [\"stable\", \"nightly\"]
            }
       ",
        )
        .expect("decoding");

        let _ck: Check = serde_json::from_str(
            "
            {
                \"type\": \"rust\",
                \"only-tip\": true,
                \"version\": \"nightly\",
                \"working-dir\": \"fuzz\",
                \"jobs\": [ \"test\", { \"fuzz\": { \"iters\": 1000000 } } ]
            }
       ",
        )
        .expect("decoding");

        let _ck: Check = serde_json::from_str(
            "
            {
                \"type\": \"rust\",
                \"only-tip\": true,
                \"version\": \"nightly\",
                \"working-dir\": \"fuzz\",
                \"jobs\": [ \"test\", { \"fuzz\": {} } ]
            }
       ",
        )
        .expect("decoding");
    }
}
