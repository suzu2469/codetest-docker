import pymysql


def get_db():
    db = pymysql.connect(
        host='localhost',
        port=3306,
        user='root',
        database='codetest',
        cursorclass=pymysql.cursors.DictCursor,
    )
    try:
        yield db
    finally:
        db.close()