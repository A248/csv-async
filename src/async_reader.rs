use std::future::Future;
use std::io;
use std::pin::Pin;
use std::result;
use std::task::{Context, Poll};

use bytes::Bytes;
use csv_core::{Reader as CoreReader, ReaderBuilder as CoreReaderBuilder};
use futures::stream::{Stream, StreamExt};

use crate::byte_record::{ByteRecord, Position};
use crate::error::{Error, ErrorKind, Result, Utf8Error};
use crate::string_record::StringRecord;
use crate::{Terminator, Trim};

/// Builds a CSV reader with various configuration knobs.
///
/// This builder can be used to tweak the field delimiter, record terminator
/// and more. Once a CSV `Reader` is built, its configuration cannot be
/// changed.
#[derive(Debug)]
pub struct AsyncReaderBuilder {
    capacity: usize,
    flexible: bool,
    has_headers: bool,
    trim: Trim,
    /// The underlying CSV parser builder.
    ///
    /// We explicitly put this on the heap because CoreReaderBuilder embeds an
    /// entire DFA transition table, which along with other things, tallies up
    /// to almost 500 bytes on the stack.
    builder: Box<CoreReaderBuilder>,
}

impl Default for AsyncReaderBuilder {
    fn default() -> Self {
        Self {
            capacity: 8 * (1 << 10),
            flexible: false,
            has_headers: true,
            trim: Trim::default(),
            builder: Box::new(CoreReaderBuilder::default()),
        }
    }
}

impl AsyncReaderBuilder {
    /// Create a new builder for configuring CSV parsing.
    ///
    /// To convert a builder into a reader, call one of the methods starting
    /// with `from_`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a CSV parser from this configuration that reads data from `rdr`.
    ///
    /// Note that the CSV reader is buffered automatically, so you should not
    /// wrap `rdr` in a buffered reader like `io::BufReader`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await}); }
    /// async fn example() {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// Concord,United States,42695
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                    ))
    ///                 )
    ///             )
    ///         );
    ///     let records = rdr
    ///         .records()
    ///         .map(Result::unwrap)
    ///         .collect::<Vec<StringRecord>>().await;
    ///     assert_eq!(records, vec![
    ///         vec!["Boston", "United States", "4628910"],
    ///         vec!["Concord", "United States", "42695"],
    ///     ]);
    /// }
    /// ```
    pub fn from_bytes_stream<S: Stream<Item = io::Result<Bytes>> + Unpin>(
        &self,
        stream: S,
    ) -> AsyncReader<S> {
        AsyncReader::new(self, stream)
    }

    /// The field delimiter to use when parsing CSV.
    ///
    /// The default is `b','`.
    ///
    /// # Example
    ///
    /// ```
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await}); }
    /// async fn example() {
    ///     let data = "\
    /// city;country;pop
    /// Boston;United States;4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .delimiter(b';')
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///     let records = rdr
    ///         .records()
    ///         .map(Result::unwrap)
    ///         .collect::<Vec<StringRecord>>().await;
    ///     assert_eq!(records, vec![
    ///         vec!["Boston", "United States", "4628910"],
    ///     ]);
    /// }
    /// ```
    pub fn delimiter(&mut self, delimiter: u8) -> &mut Self {
        self.builder.delimiter(delimiter);
        self
    }

    /// Whether to treat the first row as a special header row.
    ///
    /// By default, the first row is treated as a special header row, which
    /// means the header is never returned by any of the record reading methods
    /// or iterators. When this is disabled (`yes` set to `false`), the first
    /// row is not treated specially.
    ///
    /// Note that the `headers` and `byte_headers` methods are unaffected by
    /// whether this is set. Those methods always return the first record.
    ///
    /// # Example
    ///
    /// This example shows what happens when `has_headers` is disabled.
    /// Namely, the first row is treated just like any other row.
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .has_headers(true)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .has_headers(false)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["city", "country", "pop"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn has_headers(&mut self, yes: bool) -> &mut Self {
        self.has_headers = yes;
        self
    }

    /// Whether the number of fields in records is allowed to change or not.
    ///
    /// When disabled (which is the default), parsing CSV data will return an
    /// error if a record is found with a number of fields different from the
    /// number of fields in a previous record.
    ///
    /// When enabled, this error checking is turned off.
    ///
    /// # Example: flexible records enabled
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     // Notice that the first row is missing the population count.
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .flexible(true)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States"]);
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Example: flexible records disabled
    ///
    /// This shows the error that appears when records of unequal length
    /// are found and flexible records have been disabled (which is the
    /// default).
    ///
    /// ```
    /// use std::error::Error;
    // / use csv::{ErrorKind, ReaderBuilder};
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, StringRecord, ErrorKind};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     // Notice that the first row is missing the population count.
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .flexible(false)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///     let mut records = rdr.records();
    ///     match records.next().await {
    ///         Some(Err(err)) => match *err.kind() {
    ///             ErrorKind::UnequalLengths { expected_len, len, .. } => {
    ///                 // The header row has 3 fields...
    ///                 assert_eq!(expected_len, 3);
    ///                 // ... but the first row has only 2 fields.
    ///                 assert_eq!(len, 2);
    ///                 Ok(())
    ///             }
    ///             ref wrong => {
    ///                 Err(From::from(format!(
    ///                     "expected UnequalLengths error but got {:?}",
    ///                     wrong)))
    ///             }
    ///         }
    ///         Some(Ok(rec)) =>
    ///             Err(From::from(format!(
    ///                 "expected one errored record but got good record {:?}",
    ///                  rec))),
    ///         None =>
    ///            Err(From::from(
    ///                "expected one errored record but got none"))
    ///     }
    /// }
    /// ```
    pub fn flexible(&mut self, yes: bool) -> &mut Self {
        self.flexible = yes;
        self
    }

    /// Whether fields are trimmed of leading and trailing whitespace or not.
    ///
    /// By default, no trimming is performed. This method permits one to
    /// override that behavior and choose one of the following options:
    ///
    /// 1. `Trim::Headers` trims only header values.
    /// 2. `Trim::Fields` trims only non-header or "field" values.
    /// 3. `Trim::All` trims both header and non-header values.
    ///
    /// A value is only interpreted as a header value if this CSV reader is
    /// configured to read a header record (which is the default).
    ///
    /// When reading string records, characters meeting the definition of
    /// Unicode whitespace are trimmed. When reading byte records, characters
    /// meeting the definition of ASCII whitespace are trimmed. ASCII
    /// whitespace characters correspond to the set `[\t\n\v\f\r ]`.
    ///
    /// # Example
    ///
    /// This example shows what happens when all values are trimmed.
    ///
    /// ```
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, StringRecord, Trim};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await}); }
    /// async fn example() {
    ///     let data = "\
    /// city ,   country ,  pop
    /// Boston,\"
    ///    United States\",4628910
    /// Concord,   United States   ,42695
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .trim(Trim::All)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///     let records = rdr
    ///         .records()
    ///         .map(Result::unwrap)
    ///         .collect::<Vec<StringRecord>>().await;
    ///     assert_eq!(records, vec![
    ///         vec!["Boston", "United States", "4628910"],
    ///         vec!["Concord", "United States", "42695"],
    ///     ]);
    /// }
    /// ```
    pub fn trim(&mut self, trim: Trim) -> &mut Self {
        self.trim = trim;
        self
    }

    /// The record terminator to use when parsing CSV.
    ///
    /// A record terminator can be any single byte. The default is a special
    /// value, `Terminator::CRLF`, which treats any occurrence of `\r`, `\n`
    /// or `\r\n` as a single record terminator.
    ///
    /// # Example: `$` as a record terminator
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, Terminator};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "city,country,pop$Boston,United States,4628910";
    ///
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .terminator(Terminator::Any(b'$'))
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn terminator(&mut self, term: Terminator) -> &mut Self {
        self.builder.terminator(term.to_core());
        self
    }

    /// The quote character to use when parsing CSV.
    ///
    /// The default is `b'"'`.
    ///
    /// # Example: single quotes instead of double quotes
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,'United States',4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .quote(b'\'')
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn quote(&mut self, quote: u8) -> &mut Self {
        self.builder.quote(quote);
        self
    }

    /// The escape character to use when parsing CSV.
    ///
    /// In some variants of CSV, quotes are escaped using a special escape
    /// character like `\` (instead of escaping quotes by doubling them).
    ///
    /// By default, recognizing these idiosyncratic escapes is disabled.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,\"The \\\"United\\\" States\",4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .escape(Some(b'\\'))
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "The \"United\" States", "4628910"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn escape(&mut self, escape: Option<u8>) -> &mut Self {
        self.builder.escape(escape);
        self
    }

    /// Enable double quote escapes.
    ///
    /// This is enabled by default, but it may be disabled. 
    /// When disabled, doubled quotes are not interpreted as escapes.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,\"The \"\"United\"\" States\",4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .double_quote(false)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "The \"United\"\" States\"", "4628910"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn double_quote(&mut self, yes: bool) -> &mut Self {
        self.builder.double_quote(yes);
        self
    }

    /// Enable or disable quoting.
    ///
    /// This is enabled by default, but it may be disabled. When disabled,
    /// quotes are not treated specially.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,\"The United States,4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .quoting(false)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "\"The United States", "4628910"]);
    ///
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .flexible(true)
    ///         .quoting(true)
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "The United States,4628910\n"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn quoting(&mut self, yes: bool) -> &mut Self {
        self.builder.quoting(yes);
        self
    }

    /// The comment character to use when parsing CSV.
    ///
    /// If the start of a record begins with the byte given here, then that
    /// line is ignored by the CSV parser.
    ///
    /// This is disabled by default.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// #Concord,United States,42695
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .comment(Some(b'#'))
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn comment(&mut self, comment: Option<u8>) -> &mut Self {
        self.builder.comment(comment);
        self
    }

    /// A convenience method for specifying a configuration to read ASCII
    /// delimited text.
    ///
    /// This sets the delimiter and record terminator to the ASCII unit
    /// separator (`\x1F`) and record separator (`\x1E`), respectively.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReaderBuilder, Terminator};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city\x1Fcountry\x1Fpop\x1EBoston\x1FUnited States\x1F4628910";
    ///
    ///     let mut rdr = AsyncReaderBuilder::new()
    ///         .ascii()
    ///         .from_bytes_stream(
    ///             stream::iter(
    ///                 std::iter::once(
    ///                     Ok(Bytes::from_static(
    ///                         data.as_bytes()
    ///                     ))
    ///                 )
    ///             )
    ///         );
    ///
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     Ok(())
    /// }
    /// ```
    pub fn ascii(&mut self) -> &mut Self {
        self.builder.ascii();
        self
    }

    /// Set the capacity (in bytes) of the buffer used in the CSV reader.
    /// This defaults to a reasonable setting.
    pub fn buffer_capacity(&mut self, capacity: usize) -> &mut Self {
        self.capacity = capacity;
        self
    }

    /// Enable or disable the NFA for parsing CSV.
    ///
    /// This is intended to be a debug option. The NFA is always slower than
    /// the DFA.
    #[doc(hidden)]
    pub fn nfa(&mut self, yes: bool) -> &mut Self {
        self.builder.nfa(yes);
        self
    }
}

/// A already configured CSV reader.
///
/// A CSV reader takes as input CSV data and transforms that into standard Rust
/// values. The most flexible way to read CSV data is as a sequence of records,
/// where a record is a sequence of fields and each field is a string. However,
/// a reader can also deserialize CSV data into Rust types like `i64` or
/// `(String, f64, f64, f64)` or even a custom struct automatically using
/// Serde.
///
/// # Configuration
///
/// A CSV reader has a couple convenient constructor methods like `from_path`
/// and `from_reader`. However, if you want to configure the CSV reader to use
/// a different delimiter or quote character (among many other things), then
/// you should use a [`AsyncReaderBuilder`](struct.AsyncReaderBuilder.html) to construct
/// a `AsyncReader`. For example, to change the field delimiter:
///
/// ```
/// use std::error::Error;
/// use bytes::Bytes;
/// use futures::stream::{self, StreamExt};
/// use csv_async::{AsyncReaderBuilder};
///
/// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
/// async fn example() -> Result<(), Box<dyn Error>> {
///     let data = "\
/// city;country;pop
/// Boston;United States;4628910
/// ";
///     let mut rdr = AsyncReaderBuilder::new()
///         .delimiter(b';')
///         .from_bytes_stream(
///             stream::iter(
///                 std::iter::once(
///                     Ok(Bytes::from_static(
///                         data.as_bytes()
///                     ))
///                 )
///             )
///         );
///
///     let mut records = rdr.records();
///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
///     Ok(())
/// }
/// ```
///
/// # Error handling
///
/// In general, CSV *parsing* does not ever return an error. That is, there is
/// no such thing as malformed CSV data. Instead, this reader will prioritize
/// finding a parse over rejecting CSV data that it does not understand. This
/// choice was inspired by other popular CSV parsers, but also because it is
/// pragmatic. CSV data varies wildly, so even if the CSV data is malformed,
/// it might still be possible to work with the data. In the land of CSV, there
/// is no "right" or "wrong," only "right" and "less right."
///
/// With that said, a number of errors can occur while reading CSV data:
///
/// * By default, all records in CSV data must have the same number of fields.
///   If a record is found with a different number of fields than a prior
///   record, then an error is returned. This behavior can be disabled by
///   enabling flexible parsing via the `flexible` method on
///   [`AsyncReaderBuilder`](struct.AsyncReaderBuilder.html).
/// * When reading CSV data from a resource (like a file), it is possible for
///   reading from the underlying resource to fail. This will return an error.
/// * When reading CSV data into `String` or `&str` fields (e.g., via a
///   [`StringRecord`](struct.StringRecord.html)), UTF-8 is strictly
///   enforced. If CSV data is invalid UTF-8, then an error is returned. If
///   you want to read invalid UTF-8, then you should use the byte oriented
///   APIs such as [`ByteRecord`](struct.ByteRecord.html). If you need explicit
///   support for another encoding entirely, then you'll need to use another
///   crate to transcode your CSV data to UTF-8 before parsing it.
/// * When using Serde to deserialize CSV data into Rust types, it is possible
///   for a number of additional errors to occur. For example, deserializing
///   a field `xyz` into an `i32` field will result in an error.
///
/// For more details on the precise semantics of errors, see the
/// [`Error`](enum.Error.html) type.
#[derive(Debug)]
pub struct AsyncReader<S>
where
    S: Stream<Item = io::Result<Bytes>>,
{
    /// The underlying CSV parser.
    ///
    /// We explicitly put this on the heap because CoreReader embeds an entire
    /// DFA transition table, which along with other things, tallies up to
    /// almost 500 bytes on the stack.
    core: Box<CoreReader>,
    /// The underlying reader.
    stream: Option<S>,
    buf_state: AsyncBufState,
    /// Various state tracking.
    ///
    /// There is more state embedded in the `CoreReader`.
    state: AsyncReaderState,
}

#[derive(Debug)]
struct AsyncBufState {
    buf: Option<Bytes>,
    offset: usize,
}

#[derive(Debug)]
struct AsyncReaderState {
    /// When set, this contains the first row of any parsed CSV data.
    ///
    /// This is always populated, regardless of whether `has_headers` is set.
    headers: Option<Headers>,
    /// When set, the first row of parsed CSV data is excluded from things
    /// that read records, like iterators and `read_record`.
    has_headers: bool,
    /// When set, there is no restriction on the length of records. When not
    /// set, every record must have the same number of fields, or else an error
    /// is reported.
    flexible: bool,
    trim: Trim,
    /// The number of fields in the first record parsed.
    first_field_count: Option<u64>,
    /// The current position of the parser.
    ///
    /// Note that this position is only observable by callers at the start
    /// of a record. More granular positions are not supported.
    cur_pos: Position,
    /// Whether the first record has been read or not.
    first: bool,
    /// Whether the reader has been seeked or not.
    seeked: bool,
    /// Whether EOF of the underlying reader has been reached or not.
    eof: bool,
}

/// Headers encapsulates any data associated with the headers of CSV data.
///
/// The headers always correspond to the first row.
#[derive(Debug)]
struct Headers {
    /// The header, as raw bytes.
    byte_record: ByteRecord,
    /// The header, as valid UTF-8 (or a UTF-8 error).
    string_record: result::Result<StringRecord, Utf8Error>,
}


impl<S> AsyncReader<S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    /// Create a new CSV reader given a builder and a source of underlying
    /// bytes.
    fn new(builder: &AsyncReaderBuilder, stream: S) -> Self {
        AsyncReader {
            core: Box::new(builder.builder.build()),
            stream: Some(stream),
            buf_state: AsyncBufState { buf: None, offset: 0 },
            state: AsyncReaderState {
                headers: None,
                has_headers: builder.has_headers,
                flexible: builder.flexible,
                trim: builder.trim,
                first_field_count: None,
                cur_pos: Position::new(),
                first: false,
                seeked: false,
                eof: false,
            },
        }
    }
    

    /// Create a new CSV parser with a default configuration for the given
    /// reader.
    ///
    /// To customize CSV parsing, use a `ReaderBuilder`.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use bytes::Bytes;
    /// use futures::stream::{self, StreamExt};
    /// use csv_async::{AsyncReader, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// Concord,United States,42695\
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Concord","United States", "42695"]);
    ///     assert!(records.next().await.is_none());
    ///     Ok(())
    /// }
    /// ```
    pub fn from_bytes_stream(stream: S) -> Self {
        AsyncReaderBuilder::new().from_bytes_stream(stream)
    }

    /// Returns a borrowed iterator over all records as strings.
    ///
    /// Each item yielded by this iterator is a `Result<StringRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::AsyncReader;
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// Concord,United States,42695\
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut records = rdr.records();
    ///     while let Some(record) = records.next().await {
    ///         println!("{:?}", record?);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn records(&mut self) -> StringRecordsStream<S> {
        StringRecordsStream::new(self)
    }

    /// Returns an owned iterator over all records as strings.
    ///
    /// Each item yielded by this iterator is a `Result<StringRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// This is mostly useful when you want to return a CSV iterator or store
    /// it somewhere.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::AsyncReader;
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut records = rdr.into_records();
    ///     while let Some(record) = records.next().await {
    ///         println!("{:?}", record?);
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn into_records<'a>(self) -> StringRecordsIntoStream<'a, S> {
        StringRecordsIntoStream::new(self)
    }

    /// Returns a borrowed iterator over all records as raw bytes.
    ///
    /// Each item yielded by this iterator is a `Result<ByteRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::AsyncReader;
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut iter = rdr.byte_records();
    ///     assert_eq!(iter.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     assert!(iter.next().await.is_none());
    ///     Ok(())
    /// }
    /// ```
    pub fn byte_records(&mut self) -> ByteRecordsStream<S> {
        ByteRecordsStream::new(self)
    }

    /// Returns an owned iterator over all records as raw bytes.
    ///
    /// Each item yielded by this iterator is a `Result<ByteRecord, Error>`.
    /// Therefore, in order to access the record, callers must handle the
    /// possibility of error (typically with `try!` or `?`).
    ///
    /// This is mostly useful when you want to return a CSV iterator or store
    /// it somewhere.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this does not include the first record.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::AsyncReader;
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut iter = rdr.into_byte_records();
    ///     assert_eq!(iter.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     assert!(iter.next().await.is_none());
    ///     Ok(())
    /// }
    /// ```
    pub fn into_byte_records<'a>(self) -> ByteRecordsIntoStream<'a, S> {
        ByteRecordsIntoStream::new(self)
    }

    /// Returns a reference to the first row read by this parser.
    ///
    /// If no row has been read yet, then this will force parsing of the first
    /// row.
    ///
    /// If there was a problem parsing the row or if it wasn't valid UTF-8,
    /// then this returns an error.
    ///
    /// If the underlying reader emits EOF before any data, then this returns
    /// an empty record.
    ///
    /// Note that this method may be used regardless of whether `has_headers`
    /// was enabled (but it is enabled by default).
    ///
    /// # Example
    ///
    /// This example shows how to get the header row of CSV data. Notice that
    /// the header row does not appear as a record in the iterator!
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::AsyncReader;
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///
    ///     // We can read the headers before iterating.
    ///     {
    ///         // `headers` borrows from the reader, so we put this in its
    ///         // own scope. That way, the borrow ends before we try iterating
    ///         // below. Alternatively, we could clone the headers.
    ///         let headers = rdr.headers().await?;
    ///         assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     }
    ///
    ///     {
    ///     let mut records = rdr.records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     assert!(records.next().await.is_none());
    ///     }
    ///
    ///     // We can also read the headers after iterating.
    ///     let headers = rdr.headers().await?;
    ///     assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     Ok(())
    /// }
    /// ```
    pub async fn headers(&mut self) -> Result<&StringRecord> {
        if self.state.headers.is_none() {
            let mut record = ByteRecord::new();
            self.read_byte_record_impl(&mut record).await?;
            self.set_headers_impl(Err(record));
        }
        let headers = self.state.headers.as_ref().unwrap();
        match headers.string_record {
            Ok(ref record) => Ok(record),
            Err(ref err) => Err(Error::new(ErrorKind::Utf8 {
                pos: headers.byte_record.position().map(Clone::clone),
                err: err.clone(),
            })),
        }
    }

    /// Returns a reference to the first row read by this parser as raw bytes.
    ///
    /// If no row has been read yet, then this will force parsing of the first
    /// row.
    ///
    /// If there was a problem parsing the row then this returns an error.
    ///
    /// If the underlying reader emits EOF before any data, then this returns
    /// an empty record.
    ///
    /// Note that this method may be used regardless of whether `has_headers`
    /// was enabled (but it is enabled by default).
    ///
    /// # Example
    ///
    /// This example shows how to get the header row of CSV data. Notice that
    /// the header row does not appear as a record in the iterator!
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::AsyncReader;
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///
    ///     // We can read the headers before iterating.
    ///     {
    ///         // `headers` borrows from the reader, so we put this in its
    ///         // own scope. That way, the borrow ends before we try iterating
    ///         // below. Alternatively, we could clone the headers.
    ///         let headers = rdr.byte_headers().await?;
    ///         assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     }
    ///
    ///     {
    ///     let mut records = rdr.byte_records();
    ///     assert_eq!(records.next().await.unwrap()?, vec!["Boston", "United States", "4628910"]);
    ///     assert!(records.next().await.is_none());
    ///     }
    ///
    ///     // We can also read the headers after iterating.
    ///     let headers = rdr.byte_headers().await?;
    ///     assert_eq!(headers, vec!["city", "country", "pop"]);
    ///     Ok(())
    /// }
    /// ```
    pub async fn byte_headers(&mut self) -> Result<&ByteRecord> {
        if self.state.headers.is_none() {
            let mut record = ByteRecord::new();
            self.read_byte_record_impl(&mut record).await?;
            self.set_headers_impl(Err(record));
        }
        Ok(&self.state.headers.as_ref().unwrap().byte_record)
    }

    /// Set the headers of this CSV parser manually.
    ///
    /// This overrides any other setting (including `set_byte_headers`). Any
    /// automatic detection of headers is disabled. This may be called at any
    /// time.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::{AsyncReader, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///
    ///     assert_eq!(rdr.headers().await?, vec!["city", "country", "pop"]);
    ///     rdr.set_headers(StringRecord::from(vec!["a", "b", "c"]));
    ///     assert_eq!(rdr.headers().await?, vec!["a", "b", "c"]);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn set_headers(&mut self, headers: StringRecord) {
        self.set_headers_impl(Ok(headers));
    }

    /// Set the headers of this CSV parser manually as raw bytes.
    ///
    /// This overrides any other setting (including `set_headers`). Any
    /// automatic detection of headers is disabled. This may be called at any
    /// time.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::{AsyncReader, ByteRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///
    ///     assert_eq!(rdr.byte_headers().await?, vec!["city", "country", "pop"]);
    ///     rdr.set_byte_headers(ByteRecord::from(vec!["a", "b", "c"]));
    ///     assert_eq!(rdr.byte_headers().await?, vec!["a", "b", "c"]);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn set_byte_headers(&mut self, headers: ByteRecord) {
        self.set_headers_impl(Err(headers));
    }

    fn set_headers_impl(
        &mut self,
        headers: result::Result<StringRecord, ByteRecord>,
    ) {
        // If we have string headers, then get byte headers. But if we have
        // byte headers, then get the string headers (or a UTF-8 error).
        let (mut str_headers, mut byte_headers) = match headers {
            Ok(string) => {
                let bytes = string.clone().into_byte_record();
                (Ok(string), bytes)
            }
            Err(bytes) => {
                match StringRecord::from_byte_record(bytes.clone()) {
                    Ok(str_headers) => (Ok(str_headers), bytes),
                    Err(err) => (Err(err.utf8_error().clone()), bytes),
                }
            }
        };
        if self.state.trim.should_trim_headers() {
            if let Ok(ref mut str_headers) = str_headers.as_mut() {
                str_headers.trim();
            }
            byte_headers.trim();
        }
        self.state.headers = Some(Headers {
            byte_record: byte_headers,
            string_record: str_headers,
        });
    }

    /// Read a single row into the given record. Returns false when no more
    /// records could be read.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this will never read the first record.
    ///
    /// This method is useful when you want to read records as fast as
    /// as possible. It's less ergonomic than an iterator, but it permits the
    /// caller to reuse the `StringRecord` allocation, which usually results
    /// in higher throughput.
    ///
    /// Records read via this method are guaranteed to have a position set
    /// on them, even if the reader is at EOF or if an error is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::{AsyncReader, StringRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut record = StringRecord::new();
    ///
    ///     if rdr.read_record(&mut record).await? {
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub async fn read_record(
        &mut self,
        record: &mut StringRecord,
    ) -> Result<bool> {
        let result = record.read_async(self).await;
        // We need to trim again because trimming string records includes
        // Unicode whitespace. (ByteRecord trimming only includes ASCII
        // whitespace.)
        if self.state.trim.should_trim_fields() {
            record.trim();
        }
        result
    }

    /// Read a single row into the given byte record. Returns false when no
    /// more records could be read.
    ///
    /// If `has_headers` was enabled via a `ReaderBuilder` (which is the
    /// default), then this will never read the first record.
    ///
    /// This method is useful when you want to read records as fast as
    /// as possible. It's less ergonomic than an iterator, but it permits the
    /// caller to reuse the `ByteRecord` allocation, which usually results
    /// in higher throughput.
    ///
    /// Records read via this method are guaranteed to have a position set
    /// on them, even if the reader is at EOF or if an error is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use std::error::Error;
    /// use futures::stream::{self, StreamExt};
    /// use bytes::Bytes;
    /// use csv_async::{AsyncReader, ByteRecord};
    ///
    /// # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    /// async fn example() -> Result<(), Box<dyn Error>> {
    ///     let data = "\
    /// city,country,pop
    /// Boston,United States,4628910
    /// ";
    ///     let mut rdr = AsyncReader::from_bytes_stream(
    ///         stream::iter(
    ///             std::iter::once(
    ///                 Ok(Bytes::from_static(
    ///                     data.as_bytes()
    ///                 ))
    ///             )
    ///         )
    ///     );
    ///     let mut record = ByteRecord::new();
    ///
    ///     if rdr.read_byte_record(&mut record).await? {
    ///         assert_eq!(record, vec!["Boston", "United States", "4628910"]);
    ///         Ok(())
    ///     } else {
    ///         Err(From::from("expected at least one record but got none"))
    ///     }
    /// }
    /// ```
    pub async fn read_byte_record(
        &mut self,
        record: &mut ByteRecord,
    ) -> Result<bool> {
        if !self.state.seeked && !self.state.has_headers && !self.state.first {
            // If the caller indicated "no headers" and we haven't yielded the
            // first record yet, then we should yield our header row if we have
            // one.
            if let Some(ref headers) = self.state.headers {
                self.state.first = true;
                record.clone_from(&headers.byte_record);
                if self.state.trim.should_trim_fields() {
                    record.trim();
                }
                return Ok(!record.is_empty());
            }
        }
        let ok = self.read_byte_record_impl(record).await?;
        self.state.first = true;
        if !self.state.seeked && self.state.headers.is_none() {
            self.set_headers_impl(Err(record.clone()));
            // If the end user indicated that we have headers, then we should
            // never return the first row. Instead, we should attempt to
            // read and return the next one.
            if self.state.has_headers {
                let result = self.read_byte_record_impl(record).await;
                if self.state.trim.should_trim_fields() {
                    record.trim();
                }
                return result;
            }
        } else if self.state.trim.should_trim_fields() {
            record.trim();
        }
        Ok(ok)
    }

    async fn fill_buf(&mut self) -> io::Result<()> {
        if self.buf_state.buf.is_some() {
            Ok(())
        } else if self.stream.is_some() {
            match self.stream.as_mut().unwrap().next().await {
                Some(bytes) => {
                    self.buf_state.buf = Some(bytes?);
                    self.buf_state.offset = 0;

                    Ok(())
                }
                None => {
                    self.stream = None;
                    self.buf_state.buf = None;
                    self.buf_state.offset = 0;

                    Ok(())
                }
            }
        } else {
            self.buf_state.buf = None;
            self.buf_state.offset = 0;

            Ok(())
        }
    }

    /// Read a byte record from the underlying CSV reader, without accounting
    /// for headers.
    #[inline(always)]
    async fn read_byte_record_impl(
        &mut self,
        record: &mut ByteRecord,
    ) -> Result<bool> {
        use csv_core::ReadRecordResult::*;

        record.clear();
        record.set_position(Some(self.state.cur_pos.clone()));
        if self.state.eof {
            return Ok(false);
        }
        let (mut outlen, mut endlen) = (0, 0);
        loop {
            let (res, nin, nout, nend) = {
                self.fill_buf().await?;

                let input = if let Some(buf) = &self.buf_state.buf {
                    &buf[self.buf_state.offset..]
                } else {
                    &[]
                };

                let (fields, ends) = record.as_parts();
                self.core.read_record(
                    input,
                    &mut fields[outlen..],
                    &mut ends[endlen..],
                )
            };

            self.buf_state.offset += nin;
            if let Some(buf) = &self.buf_state.buf {
                if self.buf_state.offset >= buf.as_ref().len() {
                    self.buf_state.buf = None;
                    self.buf_state.offset = 0;
                }
            }

            let byte = self.state.cur_pos.byte();
            self.state
                .cur_pos
                .set_byte(byte + nin as u64)
                .set_line(self.core.line());
            outlen += nout;
            endlen += nend;
            match res {
                InputEmpty => continue,
                OutputFull => {
                    record.expand_fields();
                    continue;
                }
                OutputEndsFull => {
                    record.expand_ends();
                    continue;
                }
                Record => {
                    record.set_len(endlen);
                    self.state.add_record(record)?;
                    return Ok(true);
                }
                End => {
                    self.state.eof = true;
                    return Ok(false);
                }
            }
        }
    }

    /// Return the current position of this CSV reader.
    ///
    /// The byte offset in the position returned can be used to `seek` this
    /// reader. In particular, seeking to a position returned here on the same
    /// data will result in parsing the same subsequent record.
    ///
    /// # Example: reading the position
    /// TODO
    /// ```
    // / use std::error::Error;
    // / use std::io;
    // / use futures::stream::{self, StreamExt};
    // / use bytes::Bytes;
    // / use csv_async::{AsyncReader, Position};
    // /
    // / # fn main() { async_std::task::block_on(async {example().await.unwrap()}); }
    // / async fn example() -> Result<(), Box<dyn Error>> {
    // /     let data = "\
    // / city,country,popcount
    // / Boston,United States,4628910
    // / Concord,United States,42695
    // / ";
    // /     let rdr = Reader::from_reader(io::Cursor::new(data));
    // /     let mut rdr = AsyncReader::from_bytes_stream(
    // /         stream::iter(
    // /             std::iter::once(
    // /                 Ok(Bytes::from_static(
    // /                     io::Cursor::new(data)
    // /                 ))
    // /             )
    // /         )
    // /     );
    // /     let mut iter = rdr.into_records();
    // /     let mut pos = Position::new();
    // /     loop {
    // /         // Read the position immediately before each record.
    // /         let next_pos = iter.reader().position().clone();
    // /         if iter.next().is_none() {
    // /             break;
    // /         }
    // /         pos = next_pos;
    // /     }
    // /
    // /     // `pos` should now be the position immediately before the last
    // /     // record.
    // /     assert_eq!(pos.byte(), 51);
    // /     assert_eq!(pos.line(), 3);
    // /     assert_eq!(pos.record(), 2);
    // /     Ok(())
    // / }
    // / ```
    pub fn position(&self) -> &Position {
        &self.state.cur_pos
    }

    /// Returns true if and only if this reader has been exhausted.
    ///
    /// When this returns true, no more records can be read from this reader
    /// (unless it has been seeked to another position).
    ///
    /// # Example
    /// TODO
    // / ```
    // / use std::error::Error;
    // / use std::io;
    // / use csv::{Reader, Position};
    // /
    // / # fn main() { example().unwrap(); }
    // / fn example() -> Result<(), Box<dyn Error>> {
    // /     let data = "\
    // / city,country,popcount
    // / Boston,United States,4628910
    // / Concord,United States,42695
    // / ";
    // /     let mut rdr = Reader::from_reader(io::Cursor::new(data));
    // /     assert!(!rdr.is_done());
    // /     for result in rdr.records() {
    // /         let _ = result?;
    // /     }
    // /     assert!(rdr.is_done());
    // /     Ok(())
    // / }
    // / ```
    pub fn is_done(&self) -> bool {
        self.state.eof
    }

    /// Returns true if and only if this reader has been configured to
    /// interpret the first record as a header record.
    pub fn has_headers(&self) -> bool {
        self.state.has_headers
    }
}

impl AsyncReaderState {
    #[inline(always)]
    fn add_record(&mut self, record: &ByteRecord) -> Result<()> {
        let i = self.cur_pos.record();
        self.cur_pos.set_record(i.checked_add(1).unwrap());
        if !self.flexible {
            match self.first_field_count {
                None => self.first_field_count = Some(record.len() as u64),
                Some(expected) => {
                    if record.len() as u64 != expected {
                        return Err(Error::new(ErrorKind::UnequalLengths {
                            pos: record.position().map(Clone::clone),
                            expected_len: expected,
                            len: record.len() as u64,
                        }));
                    }
                }
            }
        }
        Ok(())
    }
}

async fn read_record<S>(
    mut rdr: AsyncReader<S>,
    mut rec: StringRecord,
) -> (Option<Result<StringRecord>>, AsyncReader<S>, StringRecord)
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    let result = match rdr.read_record(&mut rec).await {
        Err(err) => Some(Err(err)),
        Ok(true) => Some(Ok(rec.clone())),
        Ok(false) => None,
    };

    (result, rdr, rec)
}

/// An owned iterator over records as strings.
pub struct StringRecordsIntoStream<'a, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'a,
{
    fut: Option<
        Pin<
            Box<
                dyn Future<
                        Output = (
                            Option<Result<StringRecord>>,
                            AsyncReader<S>,
                            StringRecord,
                        ),
                    > + 'a,
            >,
        >,
    >,
}

impl<'a, S> StringRecordsIntoStream<'a, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'a,
{
    fn new(rdr: AsyncReader<S>) -> Self {
        Self {
            fut: Some(Pin::from(Box::new(read_record(
                rdr,
                StringRecord::new(),
            )))),
        }
    }
}

impl<'a, S> Stream for StringRecordsIntoStream<'a, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'a,
{
    type Item = Result<StringRecord>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<StringRecord>>> {
        match self.fut.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Ready((result, rdr, rec)) => {
                if result.is_some() {
                    self.fut =
                        Some(Pin::from(Box::new(read_record(rdr, rec))));
                } else {
                    self.fut = None;
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn read_record_borrowed<'a, S>(
    rdr: &'a mut AsyncReader<S>,
    mut rec: StringRecord,
) -> (Option<Result<StringRecord>>, &'a mut AsyncReader<S>, StringRecord)
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    let result = match rdr.read_record(&mut rec).await {
        Err(err) => Some(Err(err)),
        Ok(true) => Some(Ok(rec.clone())),
        Ok(false) => None,
    };

    (result, rdr, rec)
}

/// A borrowed iterator over records as strings.
///
/// The lifetime parameter `'r` refers to the lifetime of the underlying
/// CSV `Reader`.
pub struct StringRecordsStream<'r, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    fut: Option<
        Pin<
            Box<
                dyn Future<
                        Output = (
                            Option<Result<StringRecord>>,
                            &'r mut AsyncReader<S>,
                            StringRecord,
                        ),
                    > + 'r,
            >,
        >,
    >,
}

impl<'r, S> StringRecordsStream<'r, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    fn new(rdr: &'r mut AsyncReader<S>) -> Self {
        Self {
            fut: Some(Pin::from(Box::new(read_record_borrowed(
                rdr,
                StringRecord::new(),
            )))),
        }
    }
}

impl<'r, S> Stream for StringRecordsStream<'r, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'r,
{
    type Item = Result<StringRecord>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<StringRecord>>> {
        match self.fut.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Ready((result, rdr, rec)) => {
                if result.is_some() {
                    self.fut = Some(Pin::from(Box::new(
                        read_record_borrowed(rdr, rec),
                    )));
                } else {
                    self.fut = None;
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn read_byte_record<S>(
    mut rdr: AsyncReader<S>,
    mut rec: ByteRecord,
) -> (Option<Result<ByteRecord>>, AsyncReader<S>, ByteRecord)
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    let result = match rdr.read_byte_record(&mut rec).await {
        Err(err) => Some(Err(err)),
        Ok(true) => Some(Ok(rec.clone())),
        Ok(false) => None,
    };

    (result, rdr, rec)
}

/// An owned iterator over records as raw bytes.
pub struct ByteRecordsIntoStream<'a, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'a,
{
    fut: Option<
        Pin<
            Box<
                dyn Future<
                        Output = (
                            Option<Result<ByteRecord>>,
                            AsyncReader<S>,
                            ByteRecord,
                        ),
                    > + 'a,
            >,
        >,
    >,
}

impl<'a, S> ByteRecordsIntoStream<'a, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'a,
{
    fn new(rdr: AsyncReader<S>) -> Self {
        Self {
            fut: Some(Pin::from(Box::new(read_byte_record(
                rdr,
                ByteRecord::new(),
            )))),
        }
    }
}

impl<'a, S> Stream for ByteRecordsIntoStream<'a, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'a,
{
    type Item = Result<ByteRecord>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<ByteRecord>>> {
        match self.fut.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Ready((result, rdr, rec)) => {
                if result.is_some() {
                    self.fut =
                        Some(Pin::from(Box::new(read_byte_record(rdr, rec))));
                } else {
                    self.fut = None;
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn read_byte_record_borrowed<'a, S>(
    rdr: &'a mut AsyncReader<S>,
    mut rec: ByteRecord,
) -> (Option<Result<ByteRecord>>, &'a mut AsyncReader<S>, ByteRecord)
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    let result = match rdr.read_byte_record(&mut rec).await {
        Err(err) => Some(Err(err)),
        Ok(true) => Some(Ok(rec.clone())),
        Ok(false) => None,
    };

    (result, rdr, rec)
}

/// A borrowed iterator over records as raw bytes.
///
/// The lifetime parameter `'r` refers to the lifetime of the underlying
/// CSV `Reader`.
pub struct ByteRecordsStream<'r, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'r,
{
    fut: Option<
        Pin<
            Box<
                dyn Future<
                        Output = (
                            Option<Result<ByteRecord>>,
                            &'r mut AsyncReader<S>,
                            ByteRecord,
                        ),
                    > + 'r,
            >,
        >,
    >,
}

impl<'r, S> ByteRecordsStream<'r, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin + 'r,
{
    fn new(rdr: &'r mut AsyncReader<S>) -> Self {
        Self {
            fut: Some(Pin::from(Box::new(read_byte_record_borrowed(
                rdr,
                ByteRecord::new(),
            )))),
        }
    }
}

impl<'r, S> Stream for ByteRecordsStream<'r, S>
where
    S: Stream<Item = io::Result<Bytes>> + Unpin,
{
    type Item = Result<ByteRecord>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<ByteRecord>>> {
        match self.fut.as_mut().unwrap().as_mut().poll(cx) {
            Poll::Ready((result, rdr, rec)) => {
                if result.is_some() {
                    self.fut = Some(Pin::from(Box::new(
                        read_byte_record_borrowed(rdr, rec),
                    )));
                } else {
                    self.fut = None;
                }

                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use async_std::task;
    use bytes::Bytes;
    use futures::stream::{self, StreamExt};

    use crate::byte_record::ByteRecord;
    use crate::error::ErrorKind;
    use crate::string_record::StringRecord;

    use super::{AsyncReaderBuilder, Position, Trim};

    async fn count(stream: impl StreamExt) -> usize {
        stream.fold(0, |acc, _| async move { acc + 1 }).await
    }

    fn b(s: &str) -> &[u8] {
        s.as_bytes()
    }
    fn s(b: &[u8]) -> &str {
        ::std::str::from_utf8(b).unwrap()
    }

    fn newpos(byte: u64, line: u64, record: u64) -> Position {
        let mut p = Position::new();
        p.set_byte(byte).set_line(line).set_record(record);
        p
    }

    #[test]
    fn read_byte_record() {
        task::block_on(async {
            let data = b("foo,\"b,ar\",baz\nabc,mno,xyz");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = ByteRecord::new();

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("foo", s(&rec[0]));
            assert_eq!("b,ar", s(&rec[1]));
            assert_eq!("baz", s(&rec[2]));

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("abc", s(&rec[0]));
            assert_eq!("mno", s(&rec[1]));
            assert_eq!("xyz", s(&rec[2]));

            assert!(!rdr.read_byte_record(&mut rec).await.unwrap());
        });
    }

    #[test]
    fn read_trimmed_records_and_headers() {
        task::block_on(async {
            let data = b("foo,  bar,\tbaz\n  1,  2,  3\n1\t,\t,3\t\t");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(true)
                .trim(Trim::All)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));

            let mut rec = ByteRecord::new();
            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!("1", s(&rec[0]));
            assert_eq!("2", s(&rec[1]));
            assert_eq!("3", s(&rec[2]));
            let mut rec = StringRecord::new();
            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!("1", &rec[0]);
            assert_eq!("", &rec[1]);
            assert_eq!("3", &rec[2]);
            {
                let headers = rdr.headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!("foo", &headers[0]);
                assert_eq!("bar", &headers[1]);
                assert_eq!("baz", &headers[2]);
            }
        });
    }

    #[test]
    fn read_trimmed_header() {
        task::block_on(async {
            let data = b("foo,  bar,\tbaz\n  1,  2,  3\n1\t,\t,3\t\t");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(true)
                .trim(Trim::Headers)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));

            let mut rec = ByteRecord::new();
            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!("  1", s(&rec[0]));
            assert_eq!("  2", s(&rec[1]));
            assert_eq!("  3", s(&rec[2]));
            {
                let headers = rdr.headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!("foo", &headers[0]);
                assert_eq!("bar", &headers[1]);
                assert_eq!("baz", &headers[2]);
            }
        });
    }

    #[test]
    fn read_trimed_header_invalid_utf8() {
        task::block_on(async {
            let data = &b"foo,  b\xFFar,\tbaz\na,b,c\nd,e,f"[..];
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(true)
                .trim(Trim::Headers)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = StringRecord::new();

            // force the headers to be read
            let _ = rdr.read_record(&mut rec).await;
            // Check the byte headers are trimmed
            {
                let headers = rdr.byte_headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!(b"foo", &headers[0]);
                assert_eq!(b"b\xFFar", &headers[1]);
                assert_eq!(b"baz", &headers[2]);
            }
            match *rdr.headers().await.unwrap_err().kind() {
                ErrorKind::Utf8 { pos: Some(ref pos), ref err } => {
                    assert_eq!(pos, &newpos(0, 1, 0));
                    assert_eq!(err.field(), 1);
                    assert_eq!(err.valid_up_to(), 3);
                }
                ref err => panic!("match failed, got {:?}", err),
            }
        });
    }

    #[test]
    fn read_trimmed_records() {
        task::block_on(async {
            let data = b("foo,  bar,\tbaz\n  1,  2,  3\n1\t,\t,3\t\t");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(true)
                .trim(Trim::Fields)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = ByteRecord::new();
            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!("1", s(&rec[0]));
            assert_eq!("2", s(&rec[1]));
            assert_eq!("3", s(&rec[2]));
            {
                let headers = rdr.headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!("foo", &headers[0]);
                assert_eq!("  bar", &headers[1]);
                assert_eq!("\tbaz", &headers[2]);
            }
        });
    }

    #[test]
    fn read_record_unequal_fails() {
        task::block_on(async {
            let data = b("foo\nbar,baz");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = ByteRecord::new();

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(1, rec.len());
            assert_eq!("foo", s(&rec[0]));

            match rdr.read_byte_record(&mut rec).await {
                Err(err) => match *err.kind() {
                    ErrorKind::UnequalLengths {
                        expected_len: 1,
                        ref pos,
                        len: 2,
                    } => {
                        assert_eq!(pos, &Some(newpos(4, 2, 1)));
                    }
                    ref wrong => panic!("match failed, got {:?}", wrong),
                },
                wrong => panic!("match failed, got {:?}", wrong),
            }
        });
    }

    #[test]
    fn read_record_unequal_ok() {
        task::block_on(async {
            let data = b("foo\nbar,baz");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = ByteRecord::new();

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(1, rec.len());
            assert_eq!("foo", s(&rec[0]));

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(2, rec.len());
            assert_eq!("bar", s(&rec[0]));
            assert_eq!("baz", s(&rec[1]));

            assert!(!rdr.read_byte_record(&mut rec).await.unwrap());
        });
    }

    // This tests that even if we get a CSV error, we can continue reading
    // if we want.
    #[test]
    fn read_record_unequal_continue() {
        task::block_on(async {
            let data = b("foo\nbar,baz\nquux");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = ByteRecord::new();

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(1, rec.len());
            assert_eq!("foo", s(&rec[0]));

            match rdr.read_byte_record(&mut rec).await {
                Err(err) => match err.kind() {
                    &ErrorKind::UnequalLengths {
                        expected_len: 1,
                        ref pos,
                        len: 2,
                    } => {
                        assert_eq!(pos, &Some(newpos(4, 2, 1)));
                    }
                    wrong => panic!("match failed, got {:?}", wrong),
                },
                wrong => panic!("match failed, got {:?}", wrong),
            }

            assert!(rdr.read_byte_record(&mut rec).await.unwrap());
            assert_eq!(1, rec.len());
            assert_eq!("quux", s(&rec[0]));

            assert!(!rdr.read_byte_record(&mut rec).await.unwrap());
        });
    }

    #[test]
    fn read_record_headers() {
        task::block_on(async {
            let data = b("foo,bar,baz\na,b,c\nd,e,f");
            let mut rdr =
                AsyncReaderBuilder::new().has_headers(true).from_bytes_stream(
                    stream::iter(iter::once(Ok(Bytes::from_static(data)))),
                );
            let mut rec = StringRecord::new();

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("a", &rec[0]);

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("d", &rec[0]);

            assert!(!rdr.read_record(&mut rec).await.unwrap());

            {
                let headers = rdr.byte_headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!(b"foo", &headers[0]);
                assert_eq!(b"bar", &headers[1]);
                assert_eq!(b"baz", &headers[2]);
            }
            {
                let headers = rdr.headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!("foo", &headers[0]);
                assert_eq!("bar", &headers[1]);
                assert_eq!("baz", &headers[2]);
            }
        });
    }

    #[test]
    fn read_record_headers_invalid_utf8() {
        task::block_on(async {
            let data = &b"foo,b\xFFar,baz\na,b,c\nd,e,f"[..];
            let mut rdr =
                AsyncReaderBuilder::new().has_headers(true).from_bytes_stream(
                    stream::iter(iter::once(Ok(Bytes::from_static(data)))),
                );
            let mut rec = StringRecord::new();

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("a", &rec[0]);

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("d", &rec[0]);

            assert!(!rdr.read_record(&mut rec).await.unwrap());

            // Check that we can read the headers as raw bytes, but that
            // if we read them as strings, we get an appropriate UTF-8 error.
            {
                let headers = rdr.byte_headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!(b"foo", &headers[0]);
                assert_eq!(b"b\xFFar", &headers[1]);
                assert_eq!(b"baz", &headers[2]);
            }
            match *rdr.headers().await.unwrap_err().kind() {
                ErrorKind::Utf8 { pos: Some(ref pos), ref err } => {
                    assert_eq!(pos, &newpos(0, 1, 0));
                    assert_eq!(err.field(), 1);
                    assert_eq!(err.valid_up_to(), 1);
                }
                ref err => panic!("match failed, got {:?}", err),
            }
        });
    }

    #[test]
    fn read_record_no_headers_before() {
        task::block_on(async {
            let data = b("foo,bar,baz\na,b,c\nd,e,f");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = StringRecord::new();

            {
                let headers = rdr.headers().await.unwrap();
                assert_eq!(3, headers.len());
                assert_eq!("foo", &headers[0]);
                assert_eq!("bar", &headers[1]);
                assert_eq!("baz", &headers[2]);
            }

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("foo", &rec[0]);

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("a", &rec[0]);

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("d", &rec[0]);

            assert!(!rdr.read_record(&mut rec).await.unwrap());
        });
    }

    #[test]
    fn read_record_no_headers_after() {
        task::block_on(async {
            let data = b("foo,bar,baz\na,b,c\nd,e,f");
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static(data),
                ))));
            let mut rec = StringRecord::new();

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("foo", &rec[0]);

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("a", &rec[0]);

            assert!(rdr.read_record(&mut rec).await.unwrap());
            assert_eq!(3, rec.len());
            assert_eq!("d", &rec[0]);

            assert!(!rdr.read_record(&mut rec).await.unwrap());

            let headers = rdr.headers().await.unwrap();
            assert_eq!(3, headers.len());
            assert_eq!("foo", &headers[0]);
            assert_eq!("bar", &headers[1]);
            assert_eq!("baz", &headers[2]);
        });
    }

    // Test that position info is reported correctly in absence of headers.
    #[test]
    fn positions_no_headers() {
        task::block_on(async {
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static("a,b,c\nx,y,z".as_bytes()),
                ))))
                .into_records();

            let pos =
                rdr.next().await.unwrap().unwrap().position().unwrap().clone();
            assert_eq!(pos.byte(), 0);
            assert_eq!(pos.line(), 1);
            assert_eq!(pos.record(), 0);

            let pos =
                rdr.next().await.unwrap().unwrap().position().unwrap().clone();
            assert_eq!(pos.byte(), 6);
            assert_eq!(pos.line(), 2);
            assert_eq!(pos.record(), 1);
        });
    }

    // Test that position info is reported correctly with headers.
    #[test]
    fn positions_headers() {
        task::block_on(async {
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(true)
                .from_bytes_stream(stream::iter(iter::once(Ok(
                    Bytes::from_static("a,b,c\nx,y,z".as_bytes()),
                ))))
                .into_records();

            let pos =
                rdr.next().await.unwrap().unwrap().position().unwrap().clone();
            assert_eq!(pos.byte(), 6);
            assert_eq!(pos.line(), 2);
            assert_eq!(pos.record(), 1);
        })
    }

    // Test that reading headers on empty data yields an empty record.
    #[test]
    fn headers_on_empty_data() {
        task::block_on(async {
            let mut rdr = AsyncReaderBuilder::new()
                .from_bytes_stream(stream::iter(iter::empty()));
            let r = rdr.byte_headers().await.unwrap();
            assert_eq!(r.len(), 0);
        });
    }

    // Test that reading the first record on empty data works.
    #[test]
    fn no_headers_on_empty_data() {
        task::block_on(async {
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::empty()));
            assert_eq!(count(rdr.records()).await, 0);
        });
    }

    // Test that reading the first record on empty data works, even if
    // we've tried to read headers before hand.
    #[test]
    fn no_headers_on_empty_data_after_headers() {
        task::block_on(async {
            let mut rdr = AsyncReaderBuilder::new()
                .has_headers(false)
                .from_bytes_stream(stream::iter(iter::empty()));
            assert_eq!(rdr.headers().await.unwrap().len(), 0);
            assert_eq!(count(rdr.records()).await, 0);
        });
    }
}