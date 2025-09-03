# Contributing to Index Maker

All contributions must follow the below guidelines.

These are standards adopted by Index Maker project, and while we appreciate
there may exist many other standards, we use these.

## Guidelines

1. Use `cargo fmt`, `cargo fix`, and `cargo autoinherit`.
1. Use `cargo clippy` for advice.
1. Use `String::from(&str)` **do not** use ~~`.to_string()`~~ and **avoid** `.into()`.
1. Use `Amount` from `symm_core::core::bits` for numeric types  **do not** use ~~`f64`~~.
1. Use `safe!(a + b).ok_or_eyre("Math problem")?` to perform math operations **do not** use raw math operations.
1. Use `Symbol` from `symm_core::core::bits` for any string that can be *Interned* (asset symbol, pair symbol, exchange symbol, operation mode).
1. Use `Address` from `symm_core::core::bits` for interaction with smart contracts.
1. Use `map()`, `filter()`, `filter_map()`, `fold()` and **avoid** `for` loops.
1. Use `partition_result()`, `map_ok()`, `fold_options()` from `itertools` to handle successes and failures and **avoid** `for` loops.
1. Use `collect()`, `collect_vec()` to produce new `HashMap<_, _>` or `Vec<_>` from iterator.
1. Use `retain()`, `retain_mut()` to remove an item or the selection of items from collection.
1. Use `eyre::Result<T>` and operator `?` and **do not** use ~~`unwrap()`~~ nor ~~`expect()`~~.
1. Use `eyre!("Failed to action you failed: {:?}", err)` formatting and **use** `: {:?}` *(colon space debug formatting)*.
1. Use `.ok_or_eyre("Failed to do your action")?` where `Option<T>` is returned.
1. Use `.map_err(|err| eyre!("Failed to do your action: {:?}", err))?` where `Result<S,E>` is returned and `?` cannot be used directly.
1. Use `tracing::warn!(%a, ?b, c = %c, d = ?d, "Failed to action you failed: {:?}", err)` when logging errors.
1. Use `tracing::info!(%a, ?b, c = %c, d = ?d, "Action you did")` when logging production level information.
1. Use `SingleObserver<T>`, `MultiObserver<T>` and follow fully asynchronous *Event Driven* design and **do not** use channels directly.
1. Use `crossbeam` channels `(some_tx, some_rx)` into `SingleObserver<T>` by calling `set_observer_from(some_tx)`.
1. Use `unbounded_traceable` channels from `symm_core::core::telemetry::crossbeam` to enable Open Telemetry.
1. Use *Facade* pattern with `Arbiter` and `Session` objects running `AsyncLoop` to interact with `async` code.
1. Use `unbounded_channel` from `tokio::sync::mpsc::unbounded` when implementing integration with `async` code.
1. Use `new()`, `try_new()`, `new_with_xxx()` to construct components, and **do not** configure components in any other way *(no magic configuration)*
1. Use `Builder` from `derive_builder` to construct components in the main application
1. Do not call *Non-Async* code directly from `async` code, and instead pass message via `SingleObserver<T>`.
1. Do not write comments unless you're providing essential information that cannot be deducted from code itself.
1. Do not write log messages unless they are needed, and chose right logging level.
1. Return `Err(eyre::Report)` and ensure errors propagate, and log errors at the top level.
1. Use AI augmentation wisely, review properly the code before submitting Pull Request **do not** make us review AI generated code.


#### NOTE ####
In all cases not covered above always follow *Rust Community* guidelines, unless found otherwise in the codebase.

# Explanation

## Use `cargo fmt`, `cargo fix`, and `cargo autoinherit` ##

*Principle:* ***Code Tidiness***

Keep codebase tidy by running:

* `cargo fmt` to keep default Cargo formatting
* `cargo fix` to clean-up the imports
* `cargo autoinherit` to ensure workspace dependencies are organised
* Always sort alphabetically all dependencies

### Settings for `cargo fmt`

We trust *unbiased* views of Rust community and we use ***only standard unmodified***
settings for formatting.

## Use `cargo clippy` for advice ##

*Principle:* ***Code Clarity***

While we are aware that *Clippy* is very useful, and we shall advise using it,
we are also aware that our codebase would need more work to pass the scrutiny
of the meticulous *Clippy*. We hope to get there at some point.

We particularly don't like *Clippy's* advice against complex tuples used by
our `SimpleSolver`, which we agree are not simple types, but we don't believe
there is a need to convert those tuples into something that would make *Clippy*
happy. We focus mainly on general correctness of the codebase and its design,
on ensuring all the flows are safe and clear rather than spending too
much time on getting everything *Clippy-happy*.

***Contributions that make Clippy happier are very welcome!*** *(as long as they
don't break the design)*

## Use `String::from(&str)`

*Principle:* ***Code Clarity***

When constructing `String` from `&str` just use `String::from("My string")`,
because this provides clarity in the code, that type being constructed is
`String`. This is to follow the core principle ***Better Explicit Than
Implicit***. As an experienced engineer I want to see the types constructed, I
don't want them to be hidden from me when I look at the code.

Of course we would use `.into()` on many occasions, and we need to find good balance as to when we can use `.into()`. There might be also temptation to use `.to_string()` at times, and that is perhaps acceptable when target is to be `String` while input is a number or other *To-Stringable* object. It is preferable to use `.to_string()` as opposed to `.into()` so that type doesn't get accidentally converted into something that is *Not-A-String*. Semantically `.into()` is seen as converting from *Some-Type-A* into *Some-Type-B*, and doesn't really tell much. Thus `.into()` can be used widely within generic traits, where exact types aren't known.

Again, always follow the principle ***More Information Is Always Better***,
however you need to ask whether it is more information or more clutter. There is
a difference between information and clutter, e.g. repeated specification of
type is clutter, while specifying type once is information.

## Use `Amount` from `symm_core::core::bits` for numeric types

*Principle:* ***Code Consistency***

Project is built on top of libraries, and types should be used following the
stack. The `symm_core::core::bits` lays on top of the stack, and that is why we
always want to use types defined in that package.

The `f64` or `f32` are binary floating point types, which are not precise enough
to be used in our application. Even if we wanted to use one of them, we would
still define a type alias. This is to guarantee type consistency across whole
codebase. The principle here is ***Keep Codebase Consistent***, which means that
every fragment of the codebase should follow same design decisions, same types
should be used, same patterns. We use `Amount` in our codebase to represent
numeric type, and all mathematical operations are performed on that type.

We specifically chosen `Decimal` implementation from `rust_decimal` package to
alias as `Amount` type. Knowing this fact we shall still use `Amount` type alias
and not `Decimal` directly. Although across codebase you will see use of
`dec!()` macro and other `Decimal` specific calls. That is all ok, but types
defines must always be `Amount` and not any other type. This allows us to
provide path for any future migration to any other decimal implementation if we
ever wanted to, as well as consistency across codebase.

## Use `safe!(a + b).ok_or_eyre("Math problem")?` macro to perform math operations

*Principle:* ***Mathematical Safety***

Across the codebase we use our home-grown `safe!()` macro to ensure that all
math is checked. We tried other similar macros out there, and they didn't
satisfy our requirements, and thus we chose to use our home-grown `safe!()`
macro instead. One small disadvantage of this macro is that `DecimalExt` needs
to be imported as well, however we don't see that as an issue, just a pattern we
repeat across our codebase.

### What does `.checked_math()` defined in `DecimalExt` do?

It is an extension, which allows us to perform math on `Option<Decimal>` type, and most
importantly allows use of `?` within any function.

```rust
    fn foo(maybe_x: Option<Amount>, maybe_y: Option<Amount>) -> eyre::Result<Amount> {
        let value = maybe_x
            .checked_math(|x| x.checked_mul(x)?.checked_add(maybe_y?))
            .ok_or_eyre("Failed to foo")?;
        Ok(value)
    }

```

We can see here an example function `foo()`, which takes `maybe_x` and `y`
values, and then it takes a square of `x` should `maybe_x` be `Some(x)`, and
after that adds `y` to the result should `maybe_y` be `Some(y)`. Note the '?'
operator. We have one `?` after `checked_mul(x)`, which returns
`Option<Amount>`, and then we have another `?` at `maybe_y`, which is of type
`Option<Amount>`. Of course the semantics of `?` is same as usual, and it causes
the function to exit returning `None`, because for functions returning
`Option<T>` when `?` encounters `None` it returns `None` from those functions.
However our function is returning `eyre::Result<Amount>` and not
`Option<Amount>`, and here the code compiles. The reason it compiles is that the
lambda inside of the `.checked_math()` return `Option<Amount>` and we use `?` in
the scope of that lambda and not scope of `foo()`. The `checked_math()` forwards
result of that lambda to the caller, and we then use `.ok_or_eyre("Math
problem")?` on it so that `Option<Amount>` is converted into
`eyre::Result<Amount>` with optional error message *"Math problem"*. Note that
across our codebase this is the exact message we use when we encounter math
problem, and that same message should continue to be used. Although sometimes we
are more explicit providing message such as *"Failed to compute x, y, z
coefficients"*. It is important to follow pattern not only when it comes to
design or code, but also error and log messages should have same structure and
same wording at all times.

### `safe!()` macro

Macro allows us to make mathematical expressions look more human readable, which is especially important where we have complex formulas.

Let's look at previous math expression:
```rust
    .checked_math(|x| x.checked_mul(x)?.checked_add(maybe_y?))
```

Using `safe!()` macro this can be rewritten as:
```rust
    .checked_math(|x| safe!(safe!(x * x) + maybe_y?))
```

We can also rewrite this as:
```rust
    .checked_math(|x| safe!(maybe_y + safe!(x * x)?))
```

Note that `safe!()` can only be used with binary operations, so ~~`safe!(x * y * z)`~~
is not possible, and instead we need to nest `safe!()`:
```rust
    safe!(safe!(x * y) * z).ok_or_eyre("Math problem")?;
```

We know that `safe!()` macro returns `Option<Amount>` and the reason why we can
pass the result of `safe!(x * y)` directly as first argument of `safe!(_ * z)`
is because of how `safe!()` macro is defined, which allows its first argument
to be either `Amount` or `Option<Amount>`.

We can have more complex expression like this:
```rust
    safe!(safe(x * y) + safe!(a * b)?
        .ok_or_eyre("Failed to multiply a times b")?)
    .ok_or_eyre("Failed to compute expression")?;
```

We can see that in this scenario things are becoming *hairy*, but we can improve
it by using `.checked_math()` extension:
```rust
    x.checked_math(|x| safe!(safe!(x * y) + safe!(a * b)?))
        .ok_or_eyre("Math problem")?;
```

So to conclude, in most cases `safe!()` macro does the job, and oftentimes you
can nest them, e.g. `safe!(safe!(a + b) * z)`, but sometimes when expression is
more complex `.checked_math()` extension becomes handy.

### IMPORTANT ###
 
*Principle:* ***Hiding of mathematical errors is inacceptable and prohibited!***

* **Never** use `unwrap()` neither `expect()` on the result of math expressions.

* The use of `unwrap_or()`, `unwrap_or_default()` is also **prohibited**.

The reason is simple: all math expressions **must** be enclosed in `safe!()`
macro, and any math failure **must** be reported back via `eyre::Result<_>`.

## Exceptional use of `.map_or()` ##

*Principle:* ***Non-Existent Element Is Expected***

In *Non-Error* cases `.map_or()` should be used to provide default value if
value can be `None`. An example of that could be when we try to find item in the
collection, and it ***is legal*** for item to not be present.

Please, **do not** use `unwrap_or()` as this immediatelly turns the red flags
on, and use `.map_or()` only in those specific exceptional cases.

### Exceptional use of `.unwrap()` in codebase ###

The `.unwrap()` **must not** be used unless there is specific situation, where
it would be ***illegal*** for `.unwrap()` to fail. We call that situation
***Infallible***.

An example case where `.unwrap()` can be permitted under condition that we
inserted an element into a collection, and now it would be a programming error
if we were unable to find it in that collection. However, this can only be
proven when insertion and find is done within narrow program scope. If
*Inballibility* cannot be proven, then it is not allowed to use `.unwrap()`.

In general **don't do it**.

### Exceptional use of `.unwrap()` in Unit Tests ###

Unit tests should use `.unwrap()` to keep test code simple and easy to follow.
There is no drawback of using `.unwrap()` in actual unit tests.


### Exceptional use of `.expect()` in main application function at startup ###

An exception can be made to permit use of `.expect()` in the `main()` program
function ***when and only when*** it an intention to terminate application due
to wrong configuration. In other words, if application cannot be started
properly `.expect()` at the top level might be used.

In examples the use of `.expect()` might be permitted, however it is better
practice to use `eyre::Result<()>`.

## Insertion to `HashMap` or `HashSet`

*Principle:* ***Code Consistency***

Use `.insert()` when suitable, followed by specific combination of calls:
```rust
    mapping.insert(key, value)
        .is_none()
        .then_some(())
        .ok_or_eyre("Duplicate key")?`;
```

This is common idiom used widely throughout our codebase. Here we insert an
element under given key, and we fail if there already was an element under that
key.

## Use `Symbol` from `symm_core::core::bits` for any string that can be *Interned* ##

*Principle:* ***Avoid Premature Pessimisation & Code Consistency***

String values that can be *Interned* should have a type of
`symm_core::core::bits::Symbol`. This type ensures that such string values are
stored efficiently without allocating endless amounts of memory for same strings.

Examples of where these should be used:
- Asset symbol (the symbol of an individual asset, e.g. `BTC`)
- Market symbol (the symbol of the traded pair, e.g. `BTCETH`)
- Exchange name (e.g. `Binance`)
- Label of the Collateral Bridge (e.g. `Arbitrum:SY100:USDC`)

#### NOTE ####

The ID types **shoud not** use `Symbol` type. The ID types should be defined using `string_id!()` macro, e.g.
```rust
    string_id!(OrderId);
    string_id!(BatchOrderId);
    string_id!(SessionId);
```

The reason is that these should be:
- distinct types, i.e. `OrderId` and `BatchOrderId` should never get mixed up in the code, and this is protected by *Type-Safety*.
- distinct values, i.e. we cannot intern ID's because each ID has different value.

Anything that can be interned should use `Symbol`, anything else should use `string_id!()` if it is an ID or `String` in all other cases.


## Use `Address` from `symm_core::core::bits` for interaction with smart contracts ##

*Principle:* ***Code Consistency***

We defined `symm_core::core::bits::Address` as a type alias to `Address` type
from `alloy` package. However to keep code consistent and also allow any
migration to different library we use type alias, which should be used across
whole codebase.


## Use `map()`, `filter()`, `filter_map()`, `fold()`, `map_ok()`, `fold_options()`, `partition_result()` ##

*Principle:* ***Code Clarity & Simplicity***

Functional programming approach is well known for its simplicity. We apply some
functional experssion to a set of parameters and we get some outcome. This is
the core of what makes it simple, because:

- We know all the inputs
- We know what output can be produces
- There is no random side-effects that we don't understand

In reality we may write semi-functional code, where we have side-effects that we
understand and expect. 

In following example we're parsing a vector of strings into a vector of numbers:

```rust
    let (good, bad): (Vec<_>, Vec<_>) = values
        .into_iter()
        .map(|x| x.parse::<Amount>())
        .partition_result();
```

We use `.map()` to map each string in the input vector into lambda that says
`|x| x.parse::<Amount>()`, which parses an `Amount` from `String` and returns
`Result<Amount, _>`. Next we use `.partition_result()` to split parsing results
into two vectors: `(good, bad): (Vec<_>, Vec<_>)`. We could have written a `for`
loop instead, however such a `for` loop would take more space, and wouldn't be
clear from first glance what is that `for` loop for. Additionally if all our code
was made of `for` loops we could easily get lost in it, which we don't want.

If we look at the above expression it is very clear what the input is: `values`,
and what the output is: A tuple of `good: Vec<_>` and `bad: Vec<_>`. It is clear
that `bad` contains errors, and if `!bad.is_empty()` we can decide what to do
with errors, and then what to do with `good` vector. Code probably follows from
that point to next expressions, so that we can easily see what are the
transformations that we are performing.

This type of constructs appear across whole codebase, and are preferred over `for`
loops. The classical `for` loops can also be found especially where there is
complex side-effects, and `?` operator is used.

## Use `eyre::Result<T>` and operator `?` ##

*Principle:* ***Code Safety & Consistency***

We use `eyre` and `thiserror` packages.

While we do use `thiserror` in few places, where we want to have specific
error variants, most of the time we use `eyre` package.

The good thing about `eyre` is that errors created using it contain information
about source line where error originally occurred, which help a lot in
debugging.  We shall always use `{:?}` formatting for `eyre::Report` error type,
for that exact reason.

Generally all functions should return `eyre::Result<T>`, because practically
all functions can fail some places. The places of failure should use `?`
operator, so that function bails with error.

For example function that would compute volley size of the order
given the price and quantity:

```rust
    fn compute_volley_size(
        &self,
        symbol: &Symbol,
        price: Amount,
        quantity: Amount,
    ) -> Result<Amount> {
        let volley_size = match self.get_price(symbol) {
            Some(price) => safe!(price * quantity).ok_or_eyre("Math problem")?,
            None => Err(eyre!("Failed to find price of: {}", symbol))?,
        };
        Ok(volley_size)
    }
```

Here we have a method that first tries to get a price of the symbol, and then
we multiple `price * quantity` keeping it safe by using `safe!()` macro. We use
`.ok_or_eyre("Math problem")?` when math expression fails, and we use
`Err(eyre!("Failed to find price of: {}", symbol))?` when we can't find the price.

The `.ok_or_eyre()` is an extension for `Option<T>` from `eyre` package, and 
the `eyre!()` is a macro from same package.

### Error Messages Consistency ###

*Principle:* ***Log Consistency***

We want to ***avoid creativity*** when it comes to error messages and any
messages that are logged or displayed. *Creativity* is good in some fields,
however here we want to have *Robotic* error messages, that all look the same
so that it is easy to analyse log files.

#### Structure of the message ####

Below is an example `eyre::Report` from previous example:

```rust
    Err(eyre!("Failed to find price of: {}", symbol))?,
```

We would prefer to have error messages start the message with phrase `Failed to`
continued by the action that we tried to perform, followed by `: ` (colon space),
and then some argument(s).

Often we needt to propagate errors, and often we receive `eyre::Report` as a result
of us calling another function. In that case we can just use `?` directly, or we
can enrish information by saying:

```rust
    .map_err(|err| Err(eyre!("Failed to load configuration file: {:?}", err)))?;
```

We would use `.map_err()` and then to stay consistent with the rest of the codebase
an argument to lambda should be named `err`, and we should format error message
using `eyre!()` and we format incoming `eyre::Report` as `{:?}`.

### Case for `thiserror` ###

Package `thiserror` is great when we have an `enum` with multiple variants,
an we want to have specific message attached to each variant formatted using
data in that variant.

For example we can see `ServerResponse` uses `thiserror` to give each
response specific message formatting based on data in that variant.
```rust
#[derive(Error, Debug)]
pub enum ServerResponse {
    #[error("NewIndexOrder: ACK [{chain_id}:{address}] {client_order_id} {timestamp}")]
    NewIndexOrderAck {
        chain_id: u32,
        address: Address,
        client_order_id: ClientOrderId,
        timestamp: DateTime<Utc>,
    },
    // ...more variants...
}
```

## Use `tracing::info!(%a, ?b, c = %c, d = ?d, "Log Message")` ###

*Principle:* ***Log Consistency & Telemetry***

We use `tracing` and integration with *Open-Telemetry*, and because of that
we require that all log messages are produced using this specific pattern:

```rust
    tracing::info!(%a, ?b, c = %x.c, d = ?x.d, "Log Message");
```

The `tracing` macros can be used in this manner that variables are passed in
as `x = stirng_value` expressions, and in the above pattern there are some
automatic expansions performed:

- `%a` expands to `a = format!("{}", a)`
- `?b` expands to `b = format!("{:?}, b)`
- `c = %x.c` expands to `c = format("{}", x.c)`
- `d = ?x.d` expands to `d = format("{:?}", x.d)`

When logging arrays we use `x = %json!(data)`, e.g.:

```rust
tracing::debug(prices = %json!(prices), "Asset Prices");
```

Correct use of `tracing` and placing variables before message as shown
above will turn those into *Open Telemetry* events that will be processed
by *Elastic Cloud*.

### Use `tracing::span!("Span Name")` ###

*Principle:* ***Log Traceability & Telemetry***

When we place `tracing::span!()` around the piece of code, then when
this code logs the message, that message has `trace_id` and `span_id`
attached to it, and we can then search in *Kibana* all messages for
given `trace_id` or `span_id`. Additionally `tracing` and *Open Telemetry*
allows us to link multiple *Spans* so that we can find linked log traces
in *Kibana*.

Below is an example use of span:

```rust
    tracing::info_span!("My Span", %x, ?y).in_scope(|| {
        run_some_function()?;
        run_another_function()
    })
```

We create new span called `"My Span"`, and we attach to it some data `x`, `y`. That
data will be present in *Kibana* when we search for all events for that span. Then
we call `run_some_function()?`, which might return some `Err`, and might log some
messages, and those messages would appear in that span we created here. Then we call
`run_another_function()`, which may also log some messages, and they will appear
in *Kibana* in that same span.

We can see that we can use spans to easily create traceable flows in the logs, that
we can then analyse in *Kibana*.


## Use `SingleObserver<T>`, `MultiObserver<T>` and follow fully asynchronous *Event Driven* design ##

*Index Maker* design is fully asynchronous meaning that every action in this system
happens as a result of an event, unlike `async` code where flow is sequential.

Because of that we have a mechanism of emitting events from commponents. The system
is *Event Driven* meaning that ***nothing*** happens on its own, there must be an 
event, and something must ***listen*** to that event to ***react*** to it. So creating
a component that emits an event does not cause other components to see that event.
We must explicitly connect other components as ***Observers***.

We use two different type of *Observer* classes:

- `SingleObserver<T>` only one observer will consume the event
- `MultiObserver<T>` multiple observers will borrow the event

You can immediatelly see that single observers consume events, while multiple
observers can only borrow the event. There is a reason for that, and that is
because if you have an event, and one observer consumes it, then any other observer
wouldn't be able to get that event, because it was already consumed. This is why
if an event is to be consumed by multiple observers, that event cannot be consumed
and instead we borrow a reference to it and share it across those multiple observers.

Commonly we use `SingleObserver<T>` in most cases, and that is because we want to
preserve the flow. By the fact that observer is consuming the event we guarantee
that the event won't be reused ever again, and that flow is always moving forwards.

There are expceptions where we use `MultiObserver<T>`, and that is when we have
event emitter with multiple subscribers, and that also means that there is no
single flow that can be followed. An example of such situation is market data
connector, which emits market data events that are then captured by other components,
but there is no actual flow from that. The fact that market data event happened
usually onlt gets registered, books & prices are updated, and no important business
logic is involved. We also use `MultiObserver<T>` for `Server` and we are not sure
if that is great idea, because `Server` emits server events that represent user
requests, and user requests should be routed to correct component rather than 
broadcasted. We are aware of that, and that can be improved in future.

### Rule of Thumb ###

*Principle:* ***Code Adaptability***

Use `SingleObserver<T>` instead of callbacks or specific channels.

We don't want to dictate to developer user which channel implementation they shall use
when they are subscribing to our events. All we know is that we emit events, and how
they get consumed is up to the developer user.

### Unit-Test Usage ###
Unit tests often attach callback function as the observer by calling:

```rust
    some_component
        .set_observer_fn(|event| match event {
            SomeEvent::Fire { .. } => { todo!("Do some assertions"); }
            SomeEvent::Water { .. }=> { todo!("Do some assertions"); }
        });
```

This is convenient way of testing that component has emitted an event without plumbing
everything with channels.

### Cross-Beam Integration ###

Channels are usefull and automatically supported.

```rust
    let (some_tx, some_rx) = crossbeam::channel::unbounded();

    some_component.set_observer_from(some_tx);
```

This is possible, because our library provides traits implementation for `crossbeam`
channels so that `.set_observer_from()` can be used with them.

### Open-Telemetry Integration ###

We also provide `unbounded_traceable` in `symm_core::core::telemetry::crossbeam` 
module, which wraps crossbeam::channel::unbounded` with *Open-Telemetry* extras,
such as creating a span for the whole event handling so that we can see in all
log messages emitted during handling of that event in *Kibana*.


