//! 多重起動の検出(S-14)。名前付きミューテックスを使う。

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SetLastError,
    WIN32_ERROR,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::{PCWSTR, Result};

use super::to_wide;

/// 多重起動の検出結果。
#[must_use]
pub enum Instance {
    /// 最初の1つ。[`InstanceLock`] を持っている間、ほかの起動からは [`Instance::AlreadyRunning`] に見える。
    First(InstanceLock),
    /// 同じ名前で、すでに別の起動が動いている。
    AlreadyRunning,
}

/// 多重起動の検出に使うミューテックスのハンドル。Drop で閉じる。
///
/// アプリが動いている間は持ち続けること(`let _ = ...` で受けると、すぐに閉じてしまう)。
#[must_use = "持っている間だけ多重起動を検出できる"]
pub struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // SAFETY: self.0 は CreateMutexW が返した有効なハンドルで、ここでだけ閉じる。
        // 閉じられなくても、プロセスの終了時に OS が閉じるので結果は見ない。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// 名前付きミューテックスで、同じ名前の起動がすでに動いているかを調べる。
///
/// `name` は `Local\` で始まる、アプリに固有の名前にする(ログオンセッションの中だけで見える)。
/// 所有権は取らない。存在するかどうかだけを見る。
///
/// 同じ名前のミューテックスに触れられない場合(管理者権限で動いている方が作ったときなど)も、
/// すでに動いているとみなす。
pub fn acquire_instance(name: &str) -> Result<Instance> {
    let name = to_wide(name);
    // CreateMutexW は既存のミューテックスを開いたときに ERROR_ALREADY_EXISTS を設定する。
    // 新しく作ったときに最後のエラーを消すとは文書に書かれていないので、先に消しておく。
    // SAFETY: スレッドの最後のエラーを設定するだけ。
    unsafe { SetLastError(WIN32_ERROR(0)) };
    // SAFETY: name は NUL 終端の UTF-16 で、呼び出しの間生きている。セキュリティ属性は既定(None)。
    let created = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) };
    // windows 0.62 の CreateMutexW は、成功時は Ok(handle) を返すだけで最後のエラーに触れない
    // (失敗時にだけ Error::from_thread で読む)。ここで読む値は CreateMutexW が設定したもの。
    // 間に最後のエラーを変える呼び出し(メモリ確保など)を挟まないこと。
    // SAFETY: スレッドの最後のエラーを読むだけ。
    let last_error = unsafe { GetLastError() };
    match created {
        Ok(handle) => {
            let lock = InstanceLock(handle);
            if last_error == ERROR_ALREADY_EXISTS {
                // 開いただけのハンドルは要らない。lock の Drop で閉じる。
                Ok(Instance::AlreadyRunning)
            } else {
                Ok(Instance::First(lock))
            }
        }
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            Ok(Instance::AlreadyRunning)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S-14: 2つ目は「動いている」、1つ目を手放したら、また最初の1つになれる。
    #[test]
    fn second_acquire_sees_first_until_released() {
        let name = "Local\\Monitoaster-test-6F1C2B7E-instance";
        let first = acquire_instance(name).expect("1つ目");
        assert!(matches!(first, Instance::First(_)));
        let second = acquire_instance(name).expect("2つ目");
        assert!(matches!(second, Instance::AlreadyRunning));
        drop(first);
        let third = acquire_instance(name).expect("手放したあと");
        assert!(matches!(third, Instance::First(_)));
    }
}
