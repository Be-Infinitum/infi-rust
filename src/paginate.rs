//! Cursor pagination.

use crate::error::Error;
use std::collections::VecDeque;
use std::future::Future;

/// One page of a listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// The cursor of the next page; `None` or empty on the last one.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, next_cursor: Option<String>) -> Self {
        Page { items, next_cursor }
    }
}

/// Iterates every item, following the cursor to the last page. Build it with
/// [`paginate`] or [`paginate_from`].
///
/// ```no_run
/// # async fn demo(client: beinfi::Client) -> Result<(), beinfi::Error> {
/// use beinfi::{paginate, Page, PaymentsListInput, PaymentsListQuery};
///
/// let client = &client;
/// let mut payments = paginate(move |cursor| async move {
///     let list = client
///         .payments()
///         .list(PaymentsListInput {
///             query: PaymentsListQuery { limit: Some(50), cursor, ..Default::default() },
///         })
///         .await?;
///     Ok(Page::new(list.data, list.next_cursor))
/// });
/// while let Some(payment) = payments.next().await {
///     let payment = payment?;
///     println!("{}", payment.id);
/// }
/// # Ok(())
/// # }
/// ```
pub struct Paginator<T, F> {
    fetch: F,
    cursor: Option<String>,
    buffer: VecDeque<T>,
    done: bool,
}

/// Iterates a listing from the first page.
pub fn paginate<T, F, Fut>(fetch: F) -> Paginator<T, F>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Page<T>, Error>>,
{
    paginate_from(None, fetch)
}

/// Like [`paginate`], starting at a cursor you already know.
pub fn paginate_from<T, F, Fut>(start: Option<String>, fetch: F) -> Paginator<T, F>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Page<T>, Error>>,
{
    Paginator {
        fetch,
        cursor: start,
        buffer: VecDeque::new(),
        done: false,
    }
}

impl<T, F, Fut> Paginator<T, F>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Page<T>, Error>>,
{
    /// The next item, or `None` after the last one. After an error the iteration stops.
    pub async fn next(&mut self) -> Option<Result<T, Error>> {
        loop {
            if let Some(item) = self.buffer.pop_front() {
                return Some(Ok(item));
            }
            if self.done {
                return None;
            }
            match (self.fetch)(self.cursor.take()).await {
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
                Ok(page) => {
                    self.buffer.extend(page.items);
                    match page.next_cursor {
                        Some(c) if !c.is_empty() => self.cursor = Some(c),
                        _ => self.done = true,
                    }
                }
            }
        }
    }

    /// Collects every remaining item, stopping at the first error.
    pub async fn try_collect(mut self) -> Result<Vec<T>, Error> {
        let mut out = vec![];
        while let Some(item) = self.next().await {
            out.push(item?);
        }
        Ok(out)
    }
}
