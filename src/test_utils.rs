use std::fmt::Debug;

use futures::Stream;
use futures::StreamExt;

pub async fn stream_timeout<O: Debug>(
    stream: impl Stream<Item = O> + Unpin,
    values: usize,
    timeout: u64,
) -> eyre::Result<()> {
    let mut sub_stream = stream.take(values);
    let f = async move {
        let mut vals = values;
        while let Some(_) = sub_stream.next().await {
            println!("recieved stream value");
            vals -= 1;
            if vals == 0 {
                break;
            }
        }
    };

    tokio::select! {
        _ = f => (),
        _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => eyre::bail!("timed out")
    };

    Ok(())
}
