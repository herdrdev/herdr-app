use std::{
    future::Future,
    net::{Shutdown, TcpStream},
    sync::Arc,
    time::Duration,
};

use rquickjs::{
    Ctx, Exception, IntoJs, Object, Promise, Result, TypedArray, Value,
    function::{Func, Opt},
    module::{Declarations, Exports, ModuleDef},
};

use smol::{
    Async, Timer, future,
    io::{self, AsyncReadExt as _, AsyncWriteExt as _},
    lock::Mutex,
};

use super::{
    super::{host, scheduler},
    connect as network_connect,
};

const IO_LIMIT: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct NetModule;

impl ModuleDef for NetModule {
    fn declare(declarations: &Declarations) -> Result<()> {
        declarations.declare("connect")?;
        declarations.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let net = Object::new(ctx.clone())?;
        net.set("connect", Func::from(connect))?;
        exports.export("connect", net.get::<_, Value>("connect")?)?;
        exports.export("default", net.into_value())?;
        Ok(())
    }
}

fn connect<'js>(ctx: Ctx<'js>, host_name: String, port: u16) -> Result<Promise<'js>> {
    let normalized = host_name.to_ascii_lowercase();
    if !host::capabilities().may_reach(&normalized) {
        return Err(Exception::throw_type(
            &ctx,
            &format!(
                "network access to `{normalized}` is not granted; add it to capabilities.network.hosts"
            ),
        ));
    }
    scheduler::blocking(&ctx, "net.connect(host, port)", move || {
        let operation = format!("{host_name}:{port}");
        let (stream, _) = network_connect::connect_tcp(&host_name, port, TIMEOUT, &operation)?;
        let stream = Async::new(stream)
            .map_err(|error| format!("setting up socket I/O for {operation} failed: {error}"))?;
        Ok(Socket {
            stream,
            read: Mutex::new(()),
            write: Mutex::new(()),
        })
    })
}

struct Socket {
    stream: Async<TcpStream>,
    read: Mutex<()>,
    write: Mutex<()>,
}

async fn socket_io<T>(work: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    future::or(work, async {
        Timer::after(TIMEOUT).await;
        Err(io::ErrorKind::TimedOut.into())
    })
    .await
}

async fn write_all(mut stream: &Async<TcpStream>, mut data: &[u8]) -> io::Result<()> {
    // As with SO_SNDTIMEO, each write that makes progress gets a new timeout.
    while !data.is_empty() {
        match socket_io(stream.write(data)).await {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => data = &data[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct ReadChunk(Option<Vec<u8>>);

impl<'js> IntoJs<'js> for ReadChunk {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        match self.0 {
            Some(bytes) => TypedArray::<u8>::new(ctx.clone(), bytes)?.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

impl<'js> IntoJs<'js> for Socket {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let socket = Object::new(ctx.clone())?;
        let connection = Arc::new(self);
        let writer = connection.clone();
        socket.set(
            "write",
            Func::from(move |ctx: Ctx<'js>, data: String| -> Result<Promise<'js>> {
                if data.len() > IO_LIMIT {
                    return Err(Exception::throw_range(
                        &ctx,
                        &format!("socket write exceeded the {IO_LIMIT} byte limit"),
                    ));
                }
                let stream = writer.clone();
                scheduler::awaiting(&ctx, "socket.write(data)", async move {
                    let _write = stream.write.lock().await;
                    write_all(&stream.stream, data.as_bytes())
                        .await
                        .map_err(|error| format!("socket write failed: {error}"))
                })
            }),
        )?;
        let reader = connection.clone();
        socket.set(
            "read",
            Func::from(
                move |ctx: Ctx<'js>, size: Opt<usize>| -> Result<Promise<'js>> {
                    let size = size.0.unwrap_or(64 * 1024);
                    if size > IO_LIMIT {
                        return Err(Exception::throw_range(
                            &ctx,
                            &format!("socket read exceeded the {IO_LIMIT} byte limit"),
                        ));
                    }
                    let stream = reader.clone();
                    scheduler::awaiting(&ctx, "socket.read(size)", async move {
                        let _read = stream.read.lock().await;
                        let mut bytes = vec![0; size];
                        let count = socket_io((&stream.stream).read(&mut bytes))
                            .await
                            .map_err(|error| format!("socket read failed: {error}"))?;
                        if count == 0 {
                            return Ok(ReadChunk(None));
                        }
                        bytes.truncate(count);
                        Ok(ReadChunk(Some(bytes)))
                    })
                },
            ),
        )?;
        socket.set(
            "close",
            Func::from(move || {
                let _ = connection.stream.get_ref().shutdown(Shutdown::Both);
            }),
        )?;
        Ok(socket.into_value())
    }
}
