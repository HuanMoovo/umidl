"""为实机验收补一条测试记录（.jpg 单图任务）。

验收脚本（verify_v12b.py）会「选仅删除记录」删掉它 —— 这是预期行为：
文件留在下载目录，记录消失。需要再跑验收时先执行本脚本重新播种。

用法：python scripts/seed_test_row.py
"""
import os
import sqlite3
import time

DB = os.path.expandvars(r'%APPDATA%\umi-downloader\umi.db')
FILE = r'D:\Media Files\Videos\umi Downloader\示例：演示视频 中文标题！第二段 后篇 先导PV [BV1Abc1234d].jpg'
ROW = {
    'id': 'testrow1',
    'title': '示例：演示视频 中文标题！第二段 后篇 先导PV',
    'url': 'https://www.bilibili.com/video/BV1Hxt66REuq',
    'thumbnail': 'http://i2.hdslb.com/bfs/archive/6049c516892ba977b3245c4104b52d6340c34f31.jpg',
    'duration': 210.0,
    'uploader': '蝶祈祈',
    'status': 'done',
    'progress': 100.0,
    'file_path': FILE,
    'format_note': '封面图片',
    'request': '{"mode":"thumb"}',
}


def main() -> None:
    if not os.path.isfile(FILE):
        raise SystemExit(f'测试文件不存在：{FILE}\n请先在前端「仅封面」下载一次，或改 FILE 指向存在的图片。')
    size = os.path.getsize(FILE)
    con = sqlite3.connect(DB)
    now = int(time.time() * 1000)
    con.execute('DELETE FROM downloads WHERE id = ?', (ROW['id'],))
    con.execute(
        'INSERT INTO downloads (id,title,url,thumbnail,duration,uploader,status,progress,'
        'downloaded,total,file_path,format_note,created_time,request) '
        'VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
        (ROW['id'], ROW['title'], ROW['url'], ROW['thumbnail'], ROW['duration'], ROW['uploader'],
         ROW['status'], ROW['progress'], size, size, ROW['file_path'], ROW['format_note'],
         now, ROW['request']),
    )
    con.commit()
    rows = list(con.execute('SELECT id, substr(title,1,16), status FROM downloads ORDER BY created_time DESC'))
    con.close()
    print(f'已播种 {ROW["id"]}（{size} 字节）；当前队列：')
    for r in rows:
        print('  ', r)


if __name__ == '__main__':
    main()
